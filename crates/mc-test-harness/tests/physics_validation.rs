//! M43 - deterministic physics validation fixtures and oracle scaffolding.
//!
//! These tests prepare repo-owned worlds from local vanilla sidecars. They do
//! not embed Mojang data and skip clearly when the sidecars are absent.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, ClientboundContainerSetSlot, ClientboundKeepAlive,
    ClientboundSetEntityData, ClientboundSetHealth, ClientboundSetTime, ClientboundSystemChat,
    ConfirmTeleportation, Direction, GameEvent, GameMode, InteractionHand, LevelChunkWithLight,
    LightUpdate, LoginPlay, MovePlayerFlags, PlayerActionKind, PlayerCommandAction, PlayerInput,
    RemoveEntities, SectionBlocksUpdate, ServerboundChangeGameMode, ServerboundChatCommand,
    ServerboundClientTickEnd, ServerboundKeepAlive, ServerboundMovePlayerPosRot,
    ServerboundPlayerAction, ServerboundPlayerCommand, ServerboundPlayerInput,
    ServerboundPlayerLoaded, ServerboundUseItemOn, SetCenterChunk, SetEntityMotion,
    SynchronizePlayerPosition, pack_block_pos, pack_section_relative_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;
use mc_world::{BlockPos, BlockRegistry, BlockStateId, WorldStorage};

const VIEW_DISTANCE: i32 = 2;

#[test]
fn deterministic_physics_fixture_materializes_named_shapes() {
    let Some(blocks) = load_block_registry() else {
        return;
    };

    let (mut world, states) = physics_fixture_world(Arc::clone(&blocks));

    assert_eq!(
        world.get_block(BlockPos { x: -4, y: 64, z: 0 }).unwrap(),
        Some(states.water),
        "fixture should contain a shallow water pool"
    );
    assert_eq!(
        world.get_block(BlockPos { x: -2, y: 66, z: 4 }).unwrap(),
        Some(states.water),
        "fixture should contain a deep swimming column"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 8, y: 63, z: 0 }).unwrap(),
        Some(states.dirt),
        "sugar cane support dirt should be stable and named"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 8, y: 65, z: 0 }).unwrap(),
        Some(states.sugar_cane),
        "sugar cane column should be present for support-break captures"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 10, y: 66, z: 0 }).unwrap(),
        Some(states.air),
        "falling-block target should leave air below the sand"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 10, y: 67, z: 0 }).unwrap(),
        Some(states.sand),
        "sand fall-start oracle should use a named vanilla sand state"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 11, y: 67, z: 0 }).unwrap(),
        Some(states.gravel),
        "gravel fall-start oracle should use a named vanilla gravel state"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 12, y: 67, z: 0 }).unwrap(),
        Some(states.anvil),
        "anvil fall-start oracle should use a named vanilla anvil state"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 10, y: 66, z: 2 }).unwrap(),
        Some(states.stone),
        "sand support-break fixture should keep support until the test breaks it"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 12, y: 67, z: 2 }).unwrap(),
        Some(states.anvil),
        "anvil support-break fixture should use the real anvil state"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 6, y: 63, z: 4 }).unwrap(),
        Some(states.farmland),
        "farmland trampling scenario should have a target block"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 6, y: 64, z: 6 }).unwrap(),
        Some(states.cactus),
        "cactus side-neighbor fixture should contain a cactus column"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 7, y: 64, z: 6 }).unwrap(),
        Some(states.air),
        "cactus side-neighbor fixture should leave placement target open"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 4, y: 64, z: 10 }).unwrap(),
        Some(states.water),
        "fluid spread oracle should include a water source"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 4, y: 64, z: -3 }).unwrap(),
        Some(states.stone),
        "full-block non-step collision target should be present"
    );
    assert_eq!(
        world.get_block(BlockPos { x: 5, y: 64, z: 10 }).unwrap(),
        Some(states.lava),
        "fluid interaction oracle should include an adjacent lava source"
    );
}

#[tokio::test]
async fn physics_fixture_server_reaches_play_and_streams_spawn_chunk() {
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, sync) = connect_to_play(addr, "M43Fixture").await;
    assert!(
        sync.y.is_finite(),
        "spawn y should be a finite server value"
    );
    drain_until_chunk(&mut client, (0, 0)).await;
}

#[tokio::test]
async fn shallow_water_entry_keeps_self_motion_client_predicted() {
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, login, _) = connect_to_play_with_login(addr, "M45ShallowWater").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: -4.8,
            y: 64.0,
            z: 0.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move into shallow water");
    client
        .write_packet(&ServerboundPlayerCommand {
            entity_id: 0,
            action: PlayerCommandAction::StartSprinting,
            data: 0,
        })
        .await
        .expect("start sprinting");
    client
        .write_packet(&ServerboundPlayerInput {
            input: PlayerInput {
                forward: true,
                jump: true,
                sprint: true,
                ..PlayerInput::default()
            },
        })
        .await
        .expect("send swim input");

    assert_no_self_authoritative_water_frames(&mut client, login.entity_id).await;
}

#[tokio::test]
async fn deep_water_swim_and_exit_keep_self_motion_client_predicted() {
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, login, _) = connect_to_play_with_login(addr, "M45WaterSelfWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: -3.0,
            y: 65.0,
            z: 4.5,
            yaw: 90.0,
            pitch: -30.0,
            flags: MovePlayerFlags::new(false, false),
        })
        .await
        .expect("move into deep water");
    client
        .write_packet(&ServerboundPlayerCommand {
            entity_id: 0,
            action: PlayerCommandAction::StartSprinting,
            data: 0,
        })
        .await
        .expect("start sprinting in deep water");
    client
        .write_packet(&ServerboundPlayerInput {
            input: PlayerInput {
                forward: true,
                jump: true,
                sprint: true,
                ..PlayerInput::default()
            },
        })
        .await
        .expect("send swim input");

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: -2.5,
            y: 65.6,
            z: 4.5,
            yaw: 90.0,
            pitch: -30.0,
            flags: MovePlayerFlags::new(false, false),
        })
        .await
        .expect("swim upward in deep water");

    assert_no_self_authoritative_water_frames(&mut client, login.entity_id).await;

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 0.5,
            y: 64.0,
            z: 4.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("leave water onto land");

    assert_no_self_authoritative_water_frames(&mut client, login.entity_id).await;
}

#[tokio::test]
async fn flat_ground_move_does_not_emit_position_correction() {
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M46FlatGround").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 0.5,
            y: 64.0,
            z: 0.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move across flat ground");

    assert_no_position_correction(&mut client).await;
}

#[tokio::test]
async fn wall_collision_corrects_player_to_last_accepted_position() {
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M46WallCollision").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 1.5,
            y: 64.0,
            z: 10.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move near wall");
    assert_no_position_correction(&mut client).await;

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 2.5,
            y: 64.0,
            z: 10.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move into wall");

    let correction = wait_for_position_correction(&mut client, Duration::from_secs(2)).await;
    assert_position_near(&correction, 1.5, 64.0, 10.5, 1.0e-6);
}

#[tokio::test]
async fn full_block_non_step_attempt_corrects_player() {
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M46NonStep").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 3.5,
            y: 64.0,
            z: -2.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move beside full block");
    assert_no_position_correction(&mut client).await;

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 4.5,
            y: 64.0,
            z: -2.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("attempt full-block step");

    let correction = wait_for_position_correction(&mut client, Duration::from_secs(2)).await;
    assert_position_near(&correction, 3.5, 64.0, -2.5, 1.0e-6);
}

#[tokio::test]
async fn landing_fall_damage_uses_accumulated_descent() {
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M46FallDamage").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChangeGameMode {
            mode: GameMode::Survival,
        })
        .await
        .expect("switch to survival");
    clientbound_frames_until_fence(&mut client, "survival mode change").await;

    for (y, on_ground) in [
        (69.0, false),
        (68.0, false),
        (67.0, false),
        (66.0, false),
        (65.0, false),
        (64.0, true),
    ] {
        client
            .write_packet(&ServerboundMovePlayerPosRot {
                x: 6.5,
                y,
                z: 0.5,
                yaw: 90.0,
                pitch: 0.0,
                flags: MovePlayerFlags::new(on_ground, false),
            })
            .await
            .expect("send fall movement");
    }

    wait_for_health_near(&mut client, 18.0, 0.01).await;
}

#[tokio::test]
async fn water_entry_suppresses_fall_damage() {
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M46WaterFall").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChangeGameMode {
            mode: GameMode::Survival,
        })
        .await
        .expect("switch to survival");
    clientbound_frames_until_fence(&mut client, "survival mode change").await;

    for y in [69.0, 68.0, 67.0, 66.0, 65.0, 64.0] {
        client
            .write_packet(&ServerboundMovePlayerPosRot {
                x: -4.5,
                y,
                z: 0.5,
                yaw: 90.0,
                pitch: 0.0,
                flags: MovePlayerFlags::new(false, false),
            })
            .await
            .expect("send water-entry movement");
    }

    assert_no_health_below(&mut client, 20.0).await;
}

#[tokio::test]
async fn sugar_cane_support_break_emits_real_block_edit_observation() {
    let Some(blocks) = load_block_registry() else {
        return;
    };
    let states = FixtureStates::resolve(&blocks);
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M43Cane").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChangeGameMode {
            mode: GameMode::Creative,
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 8.5,
            y: 64.0,
            z: 2.5,
            yaw: 180.0,
            pitch: 20.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move within support break reach");
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(8, 63, 0),
            direction: Direction::Up,
            sequence: 43,
        })
        .await
        .expect("break sugar cane support");

    let observation = wait_for_block_action_observation(
        &mut client,
        43,
        &[(8, 63, 0), (8, 64, 0), (8, 65, 0), (8, 66, 0)],
        BlockActionCompletion::BlockUpdates,
    )
    .await;
    assert_eq!(
        observation.last_target_state(),
        Some(states.flowing_water.0 as i32),
        "support block next to water should match vanilla flowing-water replacement"
    );
    for y in 64..=66 {
        assert_eq!(
            observation.last_state_at((8, y, 0)),
            Some(states.air.0 as i32),
            "sugar cane at y={y} should cascade to air when support breaks"
        );
    }
    assert!(
        observation.saw_ack,
        "block edit should acknowledge sequence"
    );
}

#[tokio::test]
async fn survival_sugar_cane_support_break_drops_cascaded_cane() {
    let Some(blocks) = load_block_registry() else {
        return;
    };
    let Some(items) = load_item_registry() else {
        return;
    };
    let states = FixtureStates::resolve(&blocks);
    let sugar_cane_item = items
        .id_of(&mc_data::Identifier::parse("minecraft:sugar_cane").unwrap())
        .expect("sugar cane item id");
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M100CaneDrop").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChangeGameMode {
            mode: GameMode::Survival,
        })
        .await
        .expect("switch to survival");
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 8.5,
            y: 64.0,
            z: 2.5,
            yaw: 180.0,
            pitch: 20.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move within support break reach");

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(8, 63, 0),
            direction: Direction::Up,
            sequence: 143,
        })
        .await
        .expect("start breaking sugar cane support");
    wait_for_world_ticks(&mut client, 32).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: pack_block_pos(8, 63, 0),
            direction: Direction::Up,
            sequence: 144,
        })
        .await
        .expect("finish breaking sugar cane support");

    let observation = wait_for_block_action_observation(
        &mut client,
        144,
        &[(8, 63, 0), (8, 64, 0), (8, 65, 0), (8, 66, 0)],
        BlockActionCompletion::SlotStack {
            item_id: sugar_cane_item,
            count: 3,
        },
    )
    .await;
    assert_eq!(
        observation.last_target_state(),
        Some(states.flowing_water.0 as i32),
        "support block next to water should become flowing water"
    );
    for y in 64..=66 {
        assert_eq!(
            observation.last_state_at((8, y, 0)),
            Some(states.air.0 as i32),
            "sugar cane at y={y} should cascade to air when support breaks"
        );
    }
    assert!(
        observation
            .slot_updates
            .iter()
            .any(|(item_id, count)| *item_id == sugar_cane_item && *count == 3),
        "support break should drop and pick up all three cascaded sugar cane items"
    );
}

#[tokio::test]
async fn falling_blocks_start_when_support_breaks() {
    let Some(blocks) = load_block_registry() else {
        return;
    };
    let states = FixtureStates::resolve(&blocks);
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M47Fall").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChangeGameMode {
            mode: GameMode::Creative,
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 11.5,
            y: 67.0,
            z: 4.5,
            yaw: 180.0,
            pitch: 35.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move within falling-block support break reach");

    for (idx, (x, falling_state)) in [(10, states.sand), (11, states.gravel), (12, states.anvil)]
        .into_iter()
        .enumerate()
    {
        let sequence = 147 + idx as i32;
        client
            .write_packet(&ServerboundPlayerAction {
                action: PlayerActionKind::StartDestroyBlock,
                position: pack_block_pos(x, 66, 2),
                direction: Direction::Up,
                sequence,
            })
            .await
            .expect("break falling-block support");
        let observation = wait_for_block_action_observation(
            &mut client,
            sequence,
            &[(x, 66, 2), (x, 67, 2)],
            BlockActionCompletion::FallingBlock {
                state_id: falling_state.0 as i32,
                x: f64::from(x) + 0.5,
                y: 67.0,
                z: 2.5,
            },
        )
        .await;
        assert_eq!(
            observation.last_state_at((x, 66, 2)),
            Some(states.air.0 as i32),
            "support cell should be removed before falling starts"
        );
        assert_eq!(
            observation.last_state_at((x, 67, 2)),
            Some(states.air.0 as i32),
            "falling block source should be cleared"
        );
        assert!(
            observation.add_entities.iter().any(|entity| {
                entity.data == falling_state.0 as i32
                    && (entity.x - (f64::from(x) + 0.5)).abs() < 0.01
                    && (entity.y - 67.0).abs() < 0.01
                    && (entity.z - 2.5).abs() < 0.01
            }),
            "falling block should spawn as AddEntity with block-state data"
        );
        assert!(
            observation.saw_ack,
            "falling-block support edit should acknowledge sequence"
        );
    }
}

#[tokio::test]
async fn stacked_falling_blocks_all_start_when_support_breaks() {
    let Some(blocks) = load_block_registry() else {
        return;
    };
    let states = FixtureStates::resolve(&blocks);
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M47FallStack").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChangeGameMode {
            mode: GameMode::Creative,
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 9.5,
            y: 67.0,
            z: 4.5,
            yaw: 180.0,
            pitch: 35.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move within stacked falling-block reach");
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(9, 66, 2),
            direction: Direction::Up,
            sequence: 150,
        })
        .await
        .expect("break stacked falling-block support");

    let observation = wait_for_block_action_observation(
        &mut client,
        150,
        &[(9, 66, 2), (9, 67, 2), (9, 68, 2)],
        BlockActionCompletion::FallingBlock {
            state_id: states.sand.0 as i32,
            x: 9.5,
            y: 68.0,
            z: 2.5,
        },
    )
    .await;
    assert_eq!(
        observation.last_state_at((9, 67, 2)),
        Some(states.air.0 as i32)
    );
    assert_eq!(
        observation.last_state_at((9, 68, 2)),
        Some(states.air.0 as i32),
        "upper sand must not remain suspended after lower sand starts falling"
    );
    for y in [67.0, 68.0] {
        assert!(
            observation.add_entities.iter().any(|entity| {
                entity.data == states.sand.0 as i32
                    && (entity.x - 9.5).abs() < 0.01
                    && (entity.y - y).abs() < 0.01
                    && (entity.z - 2.5).abs() < 0.01
            }),
            "sand at y={y} must spawn its own falling-block entity"
        );
    }
}

#[tokio::test]
async fn falling_block_lands_as_block_and_despawns_entity() {
    let Some(blocks) = load_block_registry() else {
        return;
    };
    let states = FixtureStates::resolve(&blocks);
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M100FallLand").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChangeGameMode {
            mode: GameMode::Creative,
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 11.5,
            y: 67.0,
            z: 4.5,
            yaw: 180.0,
            pitch: 35.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move within falling-block support break reach");

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(10, 66, 2),
            direction: Direction::Up,
            sequence: 150,
        })
        .await
        .expect("break sand support");
    let observation = wait_for_block_action_observation(
        &mut client,
        150,
        &[(10, 66, 2), (10, 67, 2)],
        BlockActionCompletion::FallingBlock {
            state_id: states.sand.0 as i32,
            x: 10.5,
            y: 67.0,
            z: 2.5,
        },
    )
    .await;
    let falling_entity_id = observation
        .add_entities
        .iter()
        .find(|entity| {
            entity.data == states.sand.0 as i32
                && (entity.x - 10.5).abs() < 0.01
                && (entity.y - 67.0).abs() < 0.01
                && (entity.z - 2.5).abs() < 0.01
        })
        .map(|entity| entity.entity_id)
        .expect("sand should spawn as falling block entity");

    wait_for_falling_block_landing(
        &mut client,
        falling_entity_id,
        (10, 64, 2),
        states.sand.0 as i32,
    )
    .await;
}

#[tokio::test]
async fn cactus_dirt_side_neighbor_placement_cascades_visible_column_removal() {
    let Some(blocks) = load_block_registry() else {
        return;
    };
    let Some(items) = load_item_registry() else {
        return;
    };
    let states = FixtureStates::resolve(&blocks);
    let dirt_item = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item id");
    let Some(addr) = start_physics_server().await else {
        return;
    };

    let (mut client, _) = connect_to_play(addr, "M82CactusSide").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChangeGameMode {
            mode: GameMode::Creative,
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 7.5,
            y: 64.0,
            z: 8.5,
            yaw: 180.0,
            pitch: 20.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move within cactus side-neighbor placement reach");
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut client, dirt_item, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(7, 63, 6),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 182,
        })
        .await
        .expect("place dirt beside cactus");

    let observation = wait_for_block_action_observation(
        &mut client,
        182,
        &[(7, 64, 6), (6, 64, 6), (6, 65, 6)],
        BlockActionCompletion::BlockUpdates,
    )
    .await;
    assert_eq!(
        observation.last_target_state(),
        Some(states.dirt.0 as i32),
        "placed side-neighbor should remain visible; updates={:?}",
        observation.updates
    );
    for y in 64..=65 {
        assert_eq!(
            observation.last_state_at((6, y, 6)),
            Some(states.air.0 as i32),
            "cactus at y={y} should clear when a dirt side neighbor is placed"
        );
    }
    assert!(
        observation.saw_ack,
        "cactus dirt side-neighbor placement should acknowledge sequence"
    );
}

#[tokio::test]
async fn external_vanilla_sugar_cane_support_break_oracle() {
    let Ok(addr) = std::env::var("M43_VANILLA_ADDR") else {
        eprintln!("skipping: M43_VANILLA_ADDR not set");
        return;
    };
    let addr = addr.parse().expect("M43_VANILLA_ADDR parses");
    let Some(blocks) = load_block_registry() else {
        return;
    };
    let states = FixtureStates::resolve(&blocks);

    let (mut client, _) = connect_external_to_play(addr, "M43Oracle").await;
    setup_vanilla_sugar_cane_fixture(&mut client).await;
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 8.5,
            y: 64.0,
            z: 2.5,
            yaw: 180.0,
            pitch: 20.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move within vanilla support break reach");
    client
        .write_packet(&ServerboundClientTickEnd)
        .await
        .expect("send pre-break client tick end");
    clientbound_frames_until_fence(&mut client, "pre-break movement").await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(8, 63, 0),
            direction: Direction::Up,
            sequence: 43,
        })
        .await
        .expect("break vanilla sugar cane support");
    client
        .write_packet(&ServerboundClientTickEnd)
        .await
        .expect("send break client tick end");

    let observation = wait_for_block_action_observation(
        &mut client,
        43,
        &[(8, 63, 0), (8, 64, 0), (8, 65, 0), (8, 66, 0)],
        BlockActionCompletion::BlockUpdates,
    )
    .await;
    assert_eq!(
        observation.last_target_state(),
        Some(states.flowing_water.0 as i32),
        "vanilla oracle: support cell is water-replaced after break"
    );
    for y in 64..=66 {
        assert_eq!(
            observation.last_state_at((8, y, 0)),
            Some(states.air.0 as i32),
            "vanilla oracle: sugar cane at y={y} should cascade to air"
        );
    }
    assert!(
        observation.saw_ack,
        "vanilla oracle: block edit should acknowledge sequence"
    );
}

async fn start_physics_server() -> Option<std::net::SocketAddr> {
    let vanilla_dir = vanilla_data_dir();
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
        return None;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &report,
    ));
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("registry builds"));
    let (storage, _) = physics_fixture_world(Arc::clone(&blocks));
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = mc_data::block_light::load(&block_light_path)
        .ok()
        .map(Arc::new);
    let registries_path = vanilla_dir.join("reports/registries.json");
    let items = mc_data::items::load_items_report(&registries_path)
        .map(|report| mc_data::items::ItemRegistry::from_report(&report))
        .unwrap_or_default();
    let entity_types = mc_data::entity_types::load_entity_types_report(&registries_path)
        .map(|report| mc_data::entity_types::EntityTypeRegistry::from_report(&report))
        .unwrap_or_default();

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M43 physics validation".into(),
        max_players: 4,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: Arc::new(items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts,
        entity_types: Arc::new(entity_types),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    Some(addr)
}

fn load_block_registry() -> Option<Arc<BlockRegistry>> {
    let blocks_json = vanilla_data_dir().join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
        return None;
    }
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    Some(Arc::new(
        mc_world::BlockRegistry::from_report(&report).expect("block registry builds"),
    ))
}

fn load_item_registry() -> Option<mc_data::items::ItemRegistry> {
    let registries_json = vanilla_data_dir().join("reports/registries.json");
    if !registries_json.exists() {
        eprintln!("skipping: {} missing", registries_json.display());
        return None;
    }
    Some(mc_data::items::ItemRegistry::from_report(
        &mc_data::items::load_items_report(&registries_json).expect("items report loads"),
    ))
}

fn vanilla_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
}

fn physics_fixture_world(blocks: Arc<BlockRegistry>) -> (WorldStorage, FixtureStates) {
    let states = FixtureStates::resolve(&blocks);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(43, Arc::clone(&blocks)));
    let mut world = WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);

    for x in -8..=12 {
        for z in -4..=12 {
            set(&mut world, x, 63, z, states.stone);
            for y in 64..=70 {
                set(&mut world, x, y, z, states.air);
            }
        }
    }

    for x in -5..=-1 {
        for z in -2..=1 {
            set(&mut world, x, 64, z, states.water);
        }
    }
    for x in -5..=-1 {
        for z in 3..=6 {
            for y in 64..=66 {
                set(&mut world, x, y, z, states.water);
            }
        }
    }

    for x in 1..=4 {
        for z in -2..=2 {
            set(&mut world, x, 67, z, states.stone);
        }
    }

    set(&mut world, 8, 63, 0, states.dirt);
    set(&mut world, 7, 63, 0, states.water);
    for y in 64..=66 {
        set(&mut world, 8, y, 0, states.sugar_cane);
    }

    set(&mut world, 10, 66, 0, states.air);
    set(&mut world, 10, 67, 0, states.sand);
    set(&mut world, 11, 66, 0, states.air);
    set(&mut world, 11, 67, 0, states.gravel);
    set(&mut world, 12, 66, 0, states.air);
    set(&mut world, 12, 67, 0, states.anvil);
    set(&mut world, 10, 66, 2, states.stone);
    set(&mut world, 10, 67, 2, states.sand);
    set(&mut world, 11, 66, 2, states.stone);
    set(&mut world, 11, 67, 2, states.gravel);
    set(&mut world, 12, 66, 2, states.stone);
    set(&mut world, 12, 67, 2, states.anvil);
    set(&mut world, 9, 66, 2, states.stone);
    set(&mut world, 9, 67, 2, states.sand);
    set(&mut world, 9, 68, 2, states.sand);
    set(&mut world, 6, 63, 4, states.farmland);
    set(&mut world, 6, 63, 6, states.sand);
    set(&mut world, 6, 64, 6, states.cactus);
    set(&mut world, 6, 65, 6, states.cactus);

    set(&mut world, 4, 63, 10, states.stone);
    set(&mut world, 4, 64, 10, states.water);
    set(&mut world, 5, 63, 10, states.stone);
    set(&mut world, 5, 64, 10, states.lava);
    set(&mut world, 4, 64, -3, states.stone);

    for x in -2..=2 {
        for z in 8..=12 {
            set(&mut world, x, 63, z, states.stone);
            if x == -2 || x == 2 || z == 8 || z == 12 {
                set(&mut world, x, 64, z, states.stone);
                set(&mut world, x, 65, z, states.stone);
            }
        }
    }

    (world, states)
}

fn set(world: &mut WorldStorage, x: i32, y: i32, z: i32, state: BlockStateId) {
    world
        .set_block_at(BlockPos { x, y, z }, state)
        .expect("fixture block edit succeeds");
}

#[derive(Clone, Copy)]
struct FixtureStates {
    air: BlockStateId,
    stone: BlockStateId,
    dirt: BlockStateId,
    water: BlockStateId,
    flowing_water: BlockStateId,
    sugar_cane: BlockStateId,
    cactus: BlockStateId,
    sand: BlockStateId,
    gravel: BlockStateId,
    anvil: BlockStateId,
    farmland: BlockStateId,
    lava: BlockStateId,
}

impl FixtureStates {
    fn resolve(blocks: &BlockRegistry) -> Self {
        Self {
            air: default_state(blocks, "minecraft:air"),
            stone: default_state(blocks, "minecraft:stone"),
            dirt: default_state(blocks, "minecraft:dirt"),
            water: default_state(blocks, "minecraft:water"),
            flowing_water: state_with_props(blocks, "minecraft:water", &[("level", "1")]),
            sugar_cane: default_state(blocks, "minecraft:sugar_cane"),
            cactus: default_state(blocks, "minecraft:cactus"),
            sand: default_state(blocks, "minecraft:sand"),
            gravel: default_state(blocks, "minecraft:gravel"),
            anvil: default_state(blocks, "minecraft:anvil"),
            farmland: default_state(blocks, "minecraft:farmland"),
            lava: default_state(blocks, "minecraft:lava"),
        }
    }
}

fn default_state(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&mc_data::Identifier::parse(name).expect("static identifier"))
        .unwrap_or_else(|| panic!("missing block {name}"))
        .default
}

fn state_with_props(blocks: &BlockRegistry, name: &str, props: &[(&str, &str)]) -> BlockStateId {
    let props = props
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(
            &mc_data::Identifier::parse(name).expect("static identifier"),
            &props,
        )
        .unwrap_or_else(|| panic!("missing block state {name} {props:?}"))
}

async fn connect_to_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let (client, _, sync) = connect_to_play_with_login(addr, name).await;
    (client, sync)
}

async fn connect_to_play_with_login(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, LoginPlay, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let login = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    (client, login, sync)
}

async fn drain_until_chunk(client: &mut Client, target: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drain chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            if (pkt.chunk_x, pkt.chunk_z) == target {
                return;
            }
        }
    }
}

async fn assert_no_self_authoritative_water_frames(client: &mut Client, player_entity_id: i32) {
    for frame in clientbound_frames_until_fence(client, "negative water-movement assertion").await {
        if frame.id == SetEntityMotion::ID {
            let mut body = frame.body;
            let motion = SetEntityMotion::decode(&mut body).expect("decode SetEntityMotion");
            assert_ne!(
                motion.entity_id, player_entity_id,
                "vanilla keeps local water movement client-predicted; server must not send self motion"
            );
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let data = ClientboundSetEntityData::decode(&mut body).expect("decode entity data");
            assert_ne!(
                data.entity_id, player_entity_id,
                "vanilla does not send self pose metadata during local water movement"
            );
        } else if frame.id == SynchronizePlayerPosition::ID {
            panic!("water movement window should not require a player correction");
        }
    }
}

async fn assert_no_position_correction(client: &mut Client) {
    for frame in clientbound_frames_until_fence(client, "negative correction assertion").await {
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let pkt = SynchronizePlayerPosition::decode(&mut body).expect("decode SyncPlayerPos");
            panic!("movement window should not require correction: {pkt:?}");
        }
    }
}

async fn wait_for_position_correction(
    client: &mut Client,
    duration: Duration,
) -> SynchronizePlayerPosition {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for position correction"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("wait for position correction");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            return SynchronizePlayerPosition::decode(&mut body).expect("decode SyncPlayerPos");
        }
    }
}

fn assert_position_near(
    correction: &SynchronizePlayerPosition,
    x: f64,
    y: f64,
    z: f64,
    tolerance: f64,
) {
    assert!(
        (correction.x - x).abs() <= tolerance,
        "correction x: expected {x}, got {}",
        correction.x
    );
    assert!(
        (correction.y - y).abs() <= tolerance,
        "correction y: expected {y}, got {}",
        correction.y
    );
    assert!(
        (correction.z - z).abs() <= tolerance,
        "correction z: expected {z}, got {}",
        correction.z
    );
}

async fn wait_for_health_near(client: &mut Client, health: f32, tolerance: f32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for health {health}"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("health update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode set health");
            if (pkt.health - health).abs() <= tolerance {
                return;
            }
        }
    }
}

async fn wait_for_slot_stack(client: &mut Client, item_id: u32, count: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for slot stack item={item_id} count={count}"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("slot stack update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.item_stack.item_id == item_id && pkt.item_stack.count == count {
                return;
            }
        }
    }
}

async fn wait_for_world_ticks(client: &mut Client, ticks: i64) {
    let mut baseline = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for simulation ticks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != ClientboundSetTime::ID {
            continue;
        }
        let mut body = frame.body;
        let packet = ClientboundSetTime::decode(&mut body).expect("decode SetTime");
        let start = *baseline.get_or_insert(packet.game_time);
        if packet.game_time.saturating_sub(start) >= ticks {
            return;
        }
    }
}

async fn assert_no_health_below(client: &mut Client, health: f32) {
    for frame in clientbound_frames_until_fence(client, "negative health assertion").await {
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode set health");
            assert!(
                pkt.health >= health,
                "water entry should suppress fall damage; got health {}",
                pkt.health
            );
        }
    }
}

async fn clientbound_frames_until_fence(
    client: &mut Client,
    reason: &str,
) -> Vec<mc_protocol::RawFrame> {
    client
        .write_packet(&ServerboundChatCommand {
            command: "list".into(),
        })
        .await
        .expect("send liveness probe command");

    read_until_system_chat(client, reason).await
}

async fn read_until_system_chat(client: &mut Client, reason: &str) -> Vec<mc_protocol::RawFrame> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut frames = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "{reason} timed out before liveness probe response"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("wait for liveness probe response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let _response =
                ClientboundSystemChat::decode(&mut body).expect("decode liveness probe response");
            return frames;
        }
        frames.push(frame);
    }
}

struct BlockActionObservation {
    updates: Vec<((i32, i32, i32), i32)>,
    add_entities: Vec<AddEntity>,
    slot_updates: Vec<(u32, i32)>,
    primary_target: (i32, i32, i32),
    saw_ack: bool,
}

#[derive(Clone, Copy)]
enum BlockActionCompletion {
    BlockUpdates,
    SlotStack {
        item_id: u32,
        count: i32,
    },
    FallingBlock {
        state_id: i32,
        x: f64,
        y: f64,
        z: f64,
    },
}

impl BlockActionObservation {
    fn last_target_state(&self) -> Option<i32> {
        self.last_state_at(self.primary_target)
    }

    fn last_state_at(&self, target: (i32, i32, i32)) -> Option<i32> {
        self.updates
            .iter()
            .rev()
            .find_map(|(pos, state)| (*pos == target).then_some(*state))
    }
}

async fn wait_for_block_action_observation(
    client: &mut Client,
    sequence: i32,
    targets: &[(i32, i32, i32)],
    completion: BlockActionCompletion,
) -> BlockActionObservation {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let primary_target = targets[0];
    let mut updates = Vec::new();
    let mut add_entities = Vec::new();
    let mut slot_updates = Vec::new();
    let mut saw_ack = false;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            panic!("timed out waiting for block action observation");
        }
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(now))
            .await
            .unwrap_or_else(|err| panic!("wait for block action observation: {err}"));
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            if targets.contains(&pos) {
                updates.push((pos, pkt.state_id));
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt = SectionBlocksUpdate::decode(&mut body).expect("decode SectionBlocksUpdate");
            for target in targets {
                if section_pos_matches(pkt.section_pos, *target) {
                    let relative = pack_section_relative_pos(target.0, target.1, target.2);
                    for change in &pkt.changes {
                        if change.relative_pos == relative {
                            updates.push((*target, change.state_id));
                        }
                    }
                }
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                saw_ack = true;
            }
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            add_entities.push(AddEntity::decode(&mut body).expect("decode AddEntity"));
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            slot_updates.push((pkt.item_stack.item_id, pkt.item_stack.count));
        }

        let saw_all_targets = targets
            .iter()
            .all(|target| updates.iter().any(|(position, _)| position == target));
        let saw_completion = match completion {
            BlockActionCompletion::BlockUpdates => true,
            BlockActionCompletion::SlotStack { item_id, count } => {
                slot_updates.contains(&(item_id, count))
            }
            BlockActionCompletion::FallingBlock { state_id, x, y, z } => {
                add_entities.iter().any(|entity| {
                    entity.data == state_id
                        && (entity.x - x).abs() < 0.01
                        && (entity.y - y).abs() < 0.01
                        && (entity.z - z).abs() < 0.01
                })
            }
        };
        if saw_ack && saw_all_targets && saw_completion {
            break;
        }
    }
    BlockActionObservation {
        updates,
        add_entities,
        slot_updates,
        primary_target,
        saw_ack,
    }
}

async fn wait_for_falling_block_landing(
    client: &mut Client,
    entity_id: i32,
    target: (i32, i32, i32),
    state_id: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_landing_block = false;
    let mut saw_landing_light = false;
    let mut saw_remove = false;
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for falling block landing at {target:?}, light update, and removal of entity {entity_id}"
        );
        if saw_landing_block && saw_landing_light && saw_remove {
            return;
        }
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "wait for falling block landing stalled: block={saw_landing_block}, \
                     light={saw_landing_light}, remove={saw_remove}: {err}"
                )
            });
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            saw_landing_block |=
                unpack_block_pos(pkt.position) == target && pkt.state_id == state_id;
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt = SectionBlocksUpdate::decode(&mut body).expect("decode SectionBlocksUpdate");
            if section_pos_matches(pkt.section_pos, target) {
                let relative = pack_section_relative_pos(target.0, target.1, target.2);
                saw_landing_block |= pkt
                    .changes
                    .iter()
                    .any(|change| change.relative_pos == relative && change.state_id == state_id);
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let pkt = RemoveEntities::decode(&mut body).expect("decode RemoveEntities");
            saw_remove |= pkt.entity_ids.contains(&entity_id);
        } else if frame.id == LightUpdate::ID {
            let mut body = frame.body;
            let pkt = LightUpdate::decode(&mut body).expect("decode LightUpdate");
            saw_landing_light |=
                pkt.chunk_x == target.0.div_euclid(16) && pkt.chunk_z == target.2.div_euclid(16);
        }
    }
}

fn section_pos_matches(section_pos: i64, target: (i32, i32, i32)) -> bool {
    let sx = unpack_signed_section_coord(section_pos >> 42, 22);
    let sy = unpack_signed_section_coord(section_pos, 20);
    let sz = unpack_signed_section_coord(section_pos >> 20, 22);
    sx == target.0.div_euclid(16) && sy == target.1.div_euclid(16) && sz == target.2.div_euclid(16)
}

fn unpack_signed_section_coord(value: i64, bits: u8) -> i32 {
    let mask = (1_i64 << bits) - 1;
    let sign = 1_i64 << (bits - 1);
    let value = value & mask;
    let signed = if value & sign == 0 {
        value
    } else {
        value - (1_i64 << bits)
    };
    signed as i32
}

async fn connect_external_to_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut sync = None;
    while sync.is_none() {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for external Play start");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let pkt = SynchronizePlayerPosition::decode(&mut body).expect("decode SyncPlayerPos");
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: pkt.teleport_id,
                })
                .await
                .expect("ack teleport");
            client
                .write_packet(&ServerboundPlayerLoaded)
                .await
                .expect("send player loaded");
            sync = Some(pkt);
        }
    }
    (client, sync.expect("sync observed"))
}

async fn setup_vanilla_sugar_cane_fixture(client: &mut Client) {
    let commands = [
        "gamerule doDaylightCycle false",
        "gamerule randomTickSpeed 0",
        "gamemode creative M43Oracle",
        "tp M43Oracle 8.5 64 2.5 180 20",
        "fill -8 63 -4 12 70 12 air",
        "fill -8 63 -4 12 63 12 stone",
        "setblock 8 63 0 dirt",
        "setblock 7 63 0 water",
        "setblock 8 64 0 sugar_cane",
        "setblock 8 65 0 sugar_cane",
        "setblock 8 66 0 sugar_cane",
    ];
    for command in commands {
        client
            .write_packet(&ServerboundChatCommand {
                command: command.into(),
            })
            .await
            .expect("send vanilla setup command");
        client
            .write_packet(&ServerboundClientTickEnd)
            .await
            .expect("send setup client tick end");
        read_until_system_chat(client, "vanilla setup command").await;
    }
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let mut body = body.clone();
    let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("echo KeepAlive");
    true
}
