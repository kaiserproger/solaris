use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use mc_data::{
    Identifier,
    blocks::solaris_required_blocks_report,
    items::{ItemRegistry, ItemReport},
};
use mc_protocol::frame::Compression;
use mc_protocol::packets::play::{GameMode, ItemStack, MovePlayerFlags};
use mc_world::{BlockPos, BlockStateId};

use super::inventory::PlayerInventory;
use super::movement::{
    AcceptedAbsoluteMovement, PlayerMovementAuthorityError, PlayerMovementAuthorityResources,
    PlayerMovementRejection,
};
use super::persistence::{PlayerPersistedState, XpState};
use super::session::SessionRegistry;
use super::simulation::simulation_channel_with_capacity;
use super::survival::SurvivalState;
use super::tests::{
    fluid_test_facts, fluid_test_registry, insert_fluid_test_chunk, interaction_state_for_blocks,
};
use super::{
    InteractionState, PlayerMovementIngressContext, PlayerPose, handle_accepted_absolute_movement,
    player_pose_collides_with_solid, player_pose_collides_with_solid_using_context,
    player_water_overlap,
};
use crate::play::chunk_stream::passable_block_name;

#[test]
fn water_plants_do_not_become_full_cube_collision_fallbacks() {
    for name in [
        "minecraft:kelp",
        "minecraft:kelp_plant",
        "minecraft:seagrass",
        "minecraft:tall_seagrass",
        "minecraft:bubble_column",
    ] {
        assert!(passable_block_name(name), "{name} must be passable");
    }
}

#[test]
fn torches_do_not_become_full_cube_collision_fallbacks() {
    for name in [
        "minecraft:torch",
        "minecraft:wall_torch",
        "minecraft:soul_torch",
        "minecraft:soul_wall_torch",
        "minecraft:redstone_torch",
        "minecraft:redstone_wall_torch",
    ] {
        assert!(passable_block_name(name), "{name} must be passable");
    }
}

#[tokio::test]
async fn swimming_pose_submerges_eyes_in_one_block_of_water() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    state
        .world
        .lock()
        .await
        .set_block_at(BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(2))
        .unwrap();

    let mut pose = PlayerPose::new(0.5, 64.0, 0.5);
    pose.swimming = true;

    assert_eq!(player_water_overlap(&state, pose).await, (true, true));
}

#[tokio::test]
async fn representative_player_geometry_boundary_matrix() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let mut state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let block_state = |name: &str| {
        state
            .blocks
            .block(&Identifier::parse(name).expect("valid movement matrix block"))
            .unwrap_or_else(|| panic!("missing movement matrix block {name}"))
            .default
    };

    state
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 0, y: 64, z: 0 },
            block_state("minecraft:stone"),
        )
        .expect("seed movement matrix ceiling");

    struct PoseCase {
        name: &'static str,
        shifting: bool,
        swimming: bool,
        body_height: f64,
        eye_height: f64,
        collides_at_ceiling_edge: bool,
    }
    for case in [
        PoseCase {
            name: "standing",
            shifting: false,
            swimming: false,
            body_height: 1.8,
            eye_height: 1.62,
            collides_at_ceiling_edge: true,
        },
        PoseCase {
            name: "crouching",
            shifting: true,
            swimming: false,
            body_height: 1.5,
            eye_height: 1.27,
            collides_at_ceiling_edge: false,
        },
        PoseCase {
            name: "swimming",
            shifting: false,
            swimming: true,
            body_height: 0.6,
            eye_height: 0.4,
            collides_at_ceiling_edge: false,
        },
    ] {
        let mut pose = PlayerPose::new(0.5, 62.21, 0.5);
        pose.shifting = case.shifting;
        pose.swimming = case.swimming;
        assert_eq!(pose.body_height(), case.body_height, "{} body", case.name);
        assert_eq!(pose.eye_height(), case.eye_height, "{} eyes", case.name);
        assert_eq!(
            player_pose_collides_with_solid(Some(&state), pose).await,
            case.collides_at_ceiling_edge,
            "{} ceiling collision boundary",
            case.name
        );
    }

    state
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 0, y: 64, z: 0 },
            block_state("minecraft:powder_snow"),
        )
        .expect("seed movement matrix powder snow");
    const LEATHER_BOOTS_ID: u32 = 1;
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:leather_boots").unwrap(),
        protocol_id: LEATHER_BOOTS_ID,
    }]));

    struct PowderCase {
        name: &'static str,
        leather_boots: bool,
        shifting: bool,
        expected_collision: bool,
    }
    for case in [
        PowderCase {
            name: "no boots sinks",
            leather_boots: false,
            shifting: false,
            expected_collision: false,
        },
        PowderCase {
            name: "leather boots stand from above",
            leather_boots: true,
            shifting: false,
            expected_collision: true,
        },
        PowderCase {
            name: "Shift descends through boots support",
            leather_boots: true,
            shifting: true,
            expected_collision: false,
        },
    ] {
        state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = if case.leather_boots {
            ItemStack::new(LEATHER_BOOTS_ID, 1)
        } else {
            ItemStack::EMPTY
        };
        let mut previous = PlayerPose::new(0.5, 65.0, 0.5);
        previous.shifting = case.shifting;
        assert_eq!(
            player_pose_collides_with_solid_using_context(
                Some(&state),
                PlayerPose::new(0.5, 64.99, 0.5),
                previous,
            )
            .await,
            case.expected_collision,
            "{}",
            case.name
        );
    }

    state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = ItemStack::EMPTY;
    for (name, y, expected_collision) in [
        ("exact falling-shape top", 64.9, false),
        ("inside falling shape", 64.89, true),
    ] {
        let mut pose = PlayerPose::new(0.5, y, 0.5);
        pose.fall_start_y = 68.0;
        assert_eq!(
            player_pose_collides_with_solid(Some(&state), pose).await,
            expected_collision,
            "{name}"
        );
    }
}

#[tokio::test]
async fn authority_movement_enforces_displacement_loaded_destination_and_world_residency() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    let authority = PlayerMovementAuthorityResources::new(
        state.world_read.clone(),
        Arc::clone(&blocks),
        Arc::clone(&state.block_facts),
    );
    let old = PlayerPose::new(0.5, 64.0, 0.5);
    let loaded = HashSet::from([(0, 0)]);

    assert_eq!(
        authority.validate_movement(
            &loaded,
            old,
            PlayerPose::new(10.5, 64.0, 0.5),
            GameMode::Survival,
            false,
        ),
        Ok(())
    );
    assert_eq!(
        authority.validate_movement(
            &loaded,
            old,
            PlayerPose::new(10.500_1, 64.0, 0.5),
            GameMode::Survival,
            false,
        ),
        Err(PlayerMovementAuthorityError::Rejected(
            PlayerMovementRejection::Displacement
        ))
    );

    let near_boundary = PlayerPose::new(15.5, 64.0, 0.5);
    let across_boundary = PlayerPose::new(16.5, 64.0, 0.5);
    assert_eq!(
        authority.validate_movement(
            &loaded,
            near_boundary,
            across_boundary,
            GameMode::Survival,
            false,
        ),
        Err(PlayerMovementAuthorityError::Rejected(
            PlayerMovementRejection::DestinationUnloaded
        ))
    );
    assert_eq!(
        authority.validate_movement(
            &HashSet::from([(0, 0), (1, 0)]),
            near_boundary,
            across_boundary,
            GameMode::Survival,
            false,
        ),
        Err(PlayerMovementAuthorityError::WorldUnavailable)
    );
}

#[tokio::test]
async fn authority_allows_in_place_non_expanding_pose_updates_before_chunk_residency() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let state = interaction_state_for_blocks(Arc::clone(&blocks));
    let authority = PlayerMovementAuthorityResources::new(
        state.world_read.clone(),
        Arc::clone(&blocks),
        Arc::clone(&state.block_facts),
    );
    let old = PlayerPose::new(0.5, 64.0, 0.5);

    let mut rotated = old;
    rotated.yaw = 90.0;
    rotated.pitch = -20.0;
    rotated.flags = MovePlayerFlags::new(true, false);
    assert_eq!(
        authority.validate_movement(&HashSet::new(), old, rotated, GameMode::Survival, false),
        Ok(())
    );

    let mut crouched = old;
    crouched.shifting = true;
    assert_eq!(
        authority.validate_movement(&HashSet::new(), old, crouched, GameMode::Survival, false),
        Ok(())
    );

    assert_eq!(
        authority.validate_movement(&HashSet::new(), crouched, old, GameMode::Survival, false,),
        Err(PlayerMovementAuthorityError::Rejected(
            PlayerMovementRejection::DestinationUnloaded
        ))
    );
}

#[tokio::test]
async fn authority_movement_sweep_rejects_tunneling_between_clear_endpoints() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    let stone = blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    state
        .world
        .lock()
        .await
        .set_block_at(BlockPos { x: 1, y: 64, z: 0 }, stone)
        .unwrap();
    let authority = PlayerMovementAuthorityResources::new(
        state.world_read.clone(),
        Arc::clone(&blocks),
        Arc::clone(&state.block_facts),
    );
    let old = PlayerPose::new(0.5, 64.0, 0.5);
    let destination = PlayerPose::new(2.5, 64.0, 0.5);

    assert!(!player_pose_collides_with_solid(Some(&state), old).await);
    assert!(!player_pose_collides_with_solid(Some(&state), destination).await);
    assert_eq!(
        authority.validate_movement(
            &HashSet::from([(0, 0)]),
            old,
            destination,
            GameMode::Survival,
            false,
        ),
        Err(PlayerMovementAuthorityError::Rejected(
            PlayerMovementRejection::SweptCollision
        ))
    );
}

#[tokio::test]
async fn authority_movement_does_not_turn_embedded_escape_into_collision_bypass() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    let stone = blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    state
        .world
        .lock()
        .await
        .set_block_at(BlockPos { x: 0, y: 64, z: 0 }, stone)
        .unwrap();
    let authority = PlayerMovementAuthorityResources::new(
        state.world_read.clone(),
        Arc::clone(&blocks),
        Arc::clone(&state.block_facts),
    );
    let old = PlayerPose::new(0.5, 64.0, 0.5);
    let still_embedded = PlayerPose::new(0.75, 64.0, 0.5);

    assert!(player_pose_collides_with_solid(Some(&state), old).await);
    assert!(player_pose_collides_with_solid(Some(&state), still_embedded).await);
    assert_eq!(
        authority.validate_movement(
            &HashSet::from([(0, 0)]),
            old,
            still_embedded,
            GameMode::Survival,
            false,
        ),
        Err(PlayerMovementAuthorityError::Rejected(
            PlayerMovementRejection::SweptCollision
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn authoritative_rejection_resyncs_client_without_advancing_local_pose() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let mut world = interaction_state_for_blocks(Arc::clone(&blocks));
    world.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&world).await;

    let registry = SessionRegistry::new();
    let profile = crate::login::LoggedInProfile {
        uuid: crate::login::offline_uuid("MovementCorrectionAlice"),
        name: "MovementCorrectionAlice".to_owned(),
    };
    let old_pose = PlayerPose::new(0.5, 64.0, 0.5);
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(8);
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        outbound_tx,
        old_pose,
    );
    registry.mark_loaded(session_id, (0, 0));
    registry.register_player_persistence(
        session_id,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(old_pose))),
    );

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    owner.configure_player_movement_authority(
        world.world_read.clone(),
        Arc::clone(&blocks),
        Arc::clone(&world.block_facts),
    );
    let simulation = handle.for_session(session_id);
    let (mut writer, _reader) = tokio::io::duplex(4096);
    let interaction: Option<&mut InteractionState> = None;
    let mut chunk_stream = None;
    let mut zone_observer = None;
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let mut player_pose = old_pose;
    let mut next_teleport_id = 0;
    let mut pending_teleport = None;
    let mut movement = Box::pin(handle_accepted_absolute_movement(
        PlayerMovementIngressContext {
            writer: &mut writer,
            compression: Compression::Disabled,
            interaction,
            chunk_stream: &mut chunk_stream,
            simulation: &simulation,
            script_zone_observer: &mut zone_observer,
            survival_state: &mut survival,
            xp_state: &mut xp,
            game_mode: GameMode::Survival,
            player_pose: &mut player_pose,
            current_tick: 7,
            next_teleport_id: &mut next_teleport_id,
            pending_teleport: &mut pending_teleport,
        },
        AcceptedAbsoluteMovement {
            x: 20.5,
            y: 64.0,
            z: 0.5,
            yaw_pitch: None,
            flags: MovePlayerFlags::new(true, false),
        },
    ));
    std::future::poll_fn(|context| {
        assert!(movement.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    movement.await.expect("movement rejection is corrected");

    assert_eq!(
        (player_pose.x, player_pose.y, player_pose.z),
        (old_pose.x, old_pose.y, old_pose.z)
    );
    assert!(pending_teleport.is_some());
    assert_eq!(next_teleport_id, 2);
}
