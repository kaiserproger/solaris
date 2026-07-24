use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_protocol::packets::play::{
    Direction, GameMode, InteractionHand, ItemStack, ServerboundUseItemOn, pack_block_pos,
};
use mc_script::{ScriptEvent, ScriptEventKind, ScriptPlayerId, script_boundary_pair};
use mc_world::{BlockPos, BlockRegistry, BlockStateId};
use tokio::sync::mpsc;

use super::*;
use crate::loader::{
    LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderClientAck, LoaderContentKind, LoaderManifest,
    LoaderPermission, LoaderPlatform, loader_block_item_model,
};
use crate::login::LoggedInProfile;
use crate::play::item_blocks::ItemToBlockTable;
use crate::play::persistence::PlayerPersistedState;
use crate::play::tests::{insert_fluid_test_chunk, interaction_state_for_blocks};
use crate::play::{
    CommandPermissions, OutboundCommand, SessionRegistration, SimulationOwner, simulation_channel,
};
use crate::server::ScriptEventSink;

const SLAB_ITEM_ID: u32 = 42;
const STAIR_ITEM_ID: u32 = 43;
const STONE_ITEM_ID: u32 = 44;
const PAPER_ITEM_ID: u32 = 45;

struct PlacementHarness {
    state: InteractionState,
    owner: SimulationOwner,
    persisted: Arc<Mutex<PlayerPersistedState>>,
    pose: PlayerPose,
    _outbound: mpsc::Receiver<OutboundCommand>,
}

async fn placement_harness(held_item: u32) -> PlacementHarness {
    placement_harness_with(ItemStack::new(held_item, 2), None).await
}

async fn placement_harness_with(
    held: ItemStack,
    loader_session: Option<crate::LoaderSession>,
) -> PlacementHarness {
    let mut reports = placement_reports();
    let loader_state_id = reports
        .iter()
        .map(|report| report.states.len() as u32)
        .sum();
    reports.push(BlockReport {
        id: Identifier::parse("example:ruby_block").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: loader_state_id,
            default: true,
            properties: BTreeMap::new(),
        }],
    });
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:oak_slab").unwrap(),
            protocol_id: SLAB_ITEM_ID,
        },
        ItemReport {
            id: Identifier::parse("minecraft:oak_stairs").unwrap(),
            protocol_id: STAIR_ITEM_ID,
        },
        ItemReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            protocol_id: STONE_ITEM_ID,
        },
        ItemReport {
            id: Identifier::parse("minecraft:paper").unwrap(),
            protocol_id: PAPER_ITEM_ID,
        },
    ]));
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.items = Arc::clone(&items);
    state.item_to_block = ItemToBlockTable::build(&items, &blocks);
    state.block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    *state.inventory.held_mut(0).unwrap() = held;
    insert_fluid_test_chunk(&state).await;

    let pose = PlayerPose::new(4.5, 64.0, 4.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("PlacementAdapter"),
        name: "PlacementAdapter".to_owned(),
    };
    let (tx, outbound) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .try_register(SessionRegistration {
            profile: &profile,
            properties: &[],
            center: (0, 0),
            view_distance: 0,
            desired: HashSet::new(),
            tx,
            pose,
            max_sessions: usize::MAX,
            script_operator: false,
            dimension: "minecraft:overworld",
            loader_session,
        })
        .unwrap();
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let persisted = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&persisted));
    let (simulation, owner) = simulation_channel();
    state.session_id = session_id;
    state.simulation = simulation.for_session(session_id);

    PlacementHarness {
        state,
        owner,
        persisted,
        pose,
        _outbound: outbound,
    }
}

fn loader_manifest() -> LoaderManifest {
    LoaderManifest {
        protocol: LOADER_PROTOCOL_VERSION,
        bundles: vec![LoaderBundle {
            owner: "example".to_owned(),
            id: "block".to_owned(),
            version: "1".to_owned(),
            artifact: "client/block.zip".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            loaders: vec![LoaderPlatform::Fabric],
            content: vec![LoaderContentKind::Blocks],
            permissions: vec![LoaderPermission::RegisterBlocks],
            cache_key: format!("example:block/1/{}", "a".repeat(64)),
            source_path: None,
            block_id: Some("example:ruby_block".to_owned()),
            block_name: Some("Ruby Block".to_owned()),
        }],
    }
}

fn loader_session(manifest: &LoaderManifest) -> crate::LoaderSession {
    manifest
        .bind_ack(&LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "test".to_owned(),
            accepted_permissions: manifest.bundles[0].permissions.clone(),
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::from([("example:ruby_block".to_owned(), 321)]),
        })
        .unwrap()
}

fn loader_stack(count: i32) -> ItemStack {
    ItemStack::new(PAPER_ITEM_ID, count)
        .with_custom_name("Ruby Block")
        .with_item_model(loader_block_item_model(0))
}

#[tokio::test]
async fn loader_block_drop_requires_ack_and_exact_canonical_state() {
    let manifest = loader_manifest();
    let acknowledged =
        placement_harness_with(loader_stack(1), Some(loader_session(&manifest))).await;
    let canonical = acknowledged
        .state
        .blocks
        .block(&Identifier::parse("example:ruby_block").unwrap())
        .unwrap()
        .default;

    assert_eq!(
        acknowledged.state.sessions.loader_block_drop_stack(
            acknowledged.state.session_id,
            canonical,
            &acknowledged.state.items,
            &acknowledged.state.blocks,
        ),
        Some(loader_stack(1))
    );
    assert_eq!(
        acknowledged.state.sessions.loader_block_drop_stack(
            acknowledged.state.session_id,
            BlockStateId(1),
            &acknowledged.state.items,
            &acknowledged.state.blocks,
        ),
        None
    );

    let unacknowledged = placement_harness_with(loader_stack(1), None).await;
    assert_eq!(
        unacknowledged.state.sessions.loader_block_drop_stack(
            unacknowledged.state.session_id,
            canonical,
            &unacknowledged.state.items,
            &unacknowledged.state.blocks,
        ),
        None
    );
}

async fn poll_placement_pending<F>(mut request: Pin<&mut F>)
where
    F: Future,
{
    std::future::poll_fn(|context| {
        assert!(
            request.as_mut().poll(context).is_pending(),
            "placement must wait for the simulation owner commit"
        );
        Poll::Ready(())
    })
    .await;
}

fn use_item_on(clicked: BlockPos, direction: Direction, cursor_y: f32) -> ServerboundUseItemOn {
    ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: pack_block_pos(clicked.x, clicked.y, clicked.z),
        direction,
        cursor_x: 0.5,
        cursor_y,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 4,
    }
}

async fn set_block(state: &InteractionState, pos: BlockPos, block: BlockStateId) {
    state.world.lock().await.set_block_at(pos, block).unwrap();
}

fn block_state(blocks: &BlockRegistry, name: &str, properties: &[(&str, &str)]) -> BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse(name).unwrap(),
            &properties
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<Vec<_>>(),
        )
        .unwrap()
}

fn assert_held_count(harness: &PlacementHarness, expected: i32) {
    let held = harness.state.inventory.held(0).unwrap();
    let expected = ItemStack::new(held.item_id, expected);
    assert_eq!(held, &expected);
    assert_eq!(
        harness.persisted.lock().unwrap().inventory.held(0),
        Some(&expected)
    );
}

fn assert_offhand_count(harness: &PlacementHarness, expected: i32) {
    let held = &harness.state.inventory.slots[PlayerInventory::OFFHAND_SLOT];
    let expected = ItemStack::new(held.item_id, expected);
    assert_eq!(held, &expected);
    assert_eq!(
        harness.persisted.lock().unwrap().inventory.slots[PlayerInventory::OFFHAND_SLOT],
        expected
    );
}

#[tokio::test]
async fn offhand_use_item_on_places_and_debits_the_packet_selected_hand() {
    let mut harness = placement_harness(STONE_ITEM_ID).await;
    harness.state.inventory.slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::new(STONE_ITEM_ID, 2);
    *harness.state.inventory.held_mut(0).unwrap() = ItemStack::EMPTY;
    harness.persisted.lock().unwrap().inventory = harness.state.inventory.clone();
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { x: 5, ..clicked };
    set_block(&harness.state, clicked, BlockStateId(1)).await;
    let mut action = use_item_on(clicked, Direction::East, 0.5);
    action.hand = InteractionHand::OffHand;

    run_accepted_placement(&mut harness, clicked, &action).await;

    assert_eq!(
        harness.state.world.lock().await.get_cached_block(target),
        Some(BlockStateId(1))
    );
    assert!(harness.state.inventory.held(0).unwrap().is_empty());
    assert_offhand_count(&harness, 1);
}

#[tokio::test]
async fn ordinary_block_placement_routes_all_six_clicked_faces() {
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    for (direction, target) in [
        (Direction::Down, BlockPos { y: 63, ..clicked }),
        (Direction::Up, BlockPos { y: 65, ..clicked }),
        (Direction::North, BlockPos { z: 3, ..clicked }),
        (Direction::South, BlockPos { z: 5, ..clicked }),
        (Direction::West, BlockPos { x: 3, ..clicked }),
        (Direction::East, BlockPos { x: 5, ..clicked }),
    ] {
        let mut harness = placement_harness(STONE_ITEM_ID).await;
        set_block(&harness.state, clicked, BlockStateId(1)).await;
        let action = use_item_on(clicked, direction, 0.5);

        run_accepted_placement(&mut harness, clicked, &action).await;

        assert_eq!(
            harness.state.world.lock().await.get_cached_block(target),
            Some(BlockStateId(1)),
            "placement did not route {direction:?} to {target:?}"
        );
        assert_held_count(&harness, 1);
    }
}

#[tokio::test]
async fn acknowledged_loader_item_places_canonical_state_and_debits_once() {
    let manifest = loader_manifest();
    let mut harness =
        placement_harness_with(loader_stack(2), Some(loader_session(&manifest))).await;
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { x: 5, ..clicked };
    set_block(&harness.state, clicked, BlockStateId(1)).await;
    let action = use_item_on(clicked, Direction::East, 0.5);

    run_accepted_placement(&mut harness, clicked, &action).await;

    let canonical = harness
        .state
        .blocks
        .block(&Identifier::parse("example:ruby_block").unwrap())
        .unwrap()
        .default;
    assert_eq!(
        harness.state.world.lock().await.get_cached_block(target),
        Some(canonical)
    );
    assert_eq!(harness.state.inventory.held(0), Some(&loader_stack(1)));
    assert_eq!(
        harness.persisted.lock().unwrap().inventory.held(0),
        Some(&loader_stack(1))
    );
}

#[tokio::test]
async fn loader_item_placement_requires_exact_model_and_acknowledged_session() {
    let manifest = loader_manifest();
    let wrong_model = ItemStack::new(PAPER_ITEM_ID, 2)
        .with_custom_name("Ruby Block")
        .with_item_model(Identifier::parse("example:not_loader").unwrap());
    for (held, loader_session) in [
        (wrong_model, Some(loader_session(&manifest))),
        (loader_stack(2), None),
        (
            ItemStack::new(STONE_ITEM_ID, 2)
                .with_custom_name("Ruby Block")
                .with_item_model(loader_block_item_model(0)),
            Some(loader_session(&manifest)),
        ),
    ] {
        let mut harness = placement_harness_with(held.clone(), loader_session).await;
        let clicked = BlockPos { x: 4, y: 64, z: 4 };
        let target = BlockPos { x: 5, ..clicked };
        set_block(&harness.state, clicked, BlockStateId(1)).await;
        let action = use_item_on(clicked, Direction::East, 0.5);
        let mut writer = Vec::new();

        handle_block_item_placement(
            &mut harness.state,
            &mut writer,
            None,
            GameMode::Survival,
            harness.pose,
            clicked,
            &action,
            (clicked.x, clicked.y, clicked.z),
        )
        .await
        .unwrap();

        assert_eq!(
            harness.state.world.lock().await.get_cached_block(target),
            Some(BlockStateId(0))
        );
        assert_eq!(harness.state.inventory.held(0), Some(&held));
        assert_eq!(
            harness.persisted.lock().unwrap().inventory.held(0),
            Some(&held)
        );
        assert_eq!(
            harness
                .owner
                .process_tick_with_world(
                    &harness.state.sessions,
                    Some(&harness.state.world),
                    None,
                    1,
                )
                .processed,
            0
        );
    }
}

#[tokio::test]
async fn ordinary_stair_placement_passes_the_real_validator_and_debits_once() {
    let mut harness = placement_harness(STAIR_ITEM_ID).await;
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { x: 5, ..clicked };
    set_block(&harness.state, clicked, BlockStateId(1)).await;
    let action = use_item_on(clicked, Direction::East, 0.5);
    let sessions = Arc::clone(&harness.state.sessions);
    let world = Arc::clone(&harness.state.world);
    let mut writer = Vec::new();
    let mut request = Box::pin(handle_block_item_placement(
        &mut harness.state,
        &mut writer,
        None,
        GameMode::Survival,
        harness.pose,
        clicked,
        &action,
        (clicked.x, clicked.y, clicked.z),
    ));

    poll_placement_pending(request.as_mut()).await;
    assert_eq!(
        harness
            .owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    request.await.unwrap();

    let placed = world.lock().await.get_cached_block(target).unwrap();
    assert_eq!(
        harness
            .state
            .blocks
            .by_id(placed)
            .unwrap()
            .block
            .id
            .as_str(),
        "minecraft:oak_stairs"
    );
    assert_held_count(&harness, 1);
}

#[tokio::test]
async fn stair_placement_commits_neighbour_shape_before_inventory_debit() {
    let mut harness = placement_harness(STAIR_ITEM_ID).await;
    harness.pose.yaw = 90.0;
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { x: 5, ..clicked };
    let north = BlockPos { z: 3, ..target };
    let existing = block_state(
        &harness.state.blocks,
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "true"),
        ],
    );
    let expected_neighbour = block_state(
        &harness.state.blocks,
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "inner_left"),
            ("waterlogged", "true"),
        ],
    );
    set_block(&harness.state, clicked, BlockStateId(1)).await;
    set_block(&harness.state, north, existing).await;
    let action = use_item_on(clicked, Direction::East, 0.5);

    run_accepted_placement(&mut harness, clicked, &action).await;

    assert_eq!(
        harness.state.world.lock().await.get_cached_block(target),
        Some(block_state(
            &harness.state.blocks,
            "minecraft:oak_stairs",
            &[
                ("facing", "west"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        ))
    );
    assert_eq!(
        harness.state.world.lock().await.get_cached_block(north),
        Some(expected_neighbour)
    );
    assert_held_count(&harness, 1);
}

#[tokio::test]
async fn stale_stair_shape_dependency_rejects_without_inventory_debit_or_script_event() {
    let mut harness = placement_harness(STAIR_ITEM_ID).await;
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { x: 5, ..clicked };
    let stale_neighbor = BlockPos { x: 6, ..target };
    set_block(&harness.state, clicked, BlockStateId(1)).await;
    let action = use_item_on(clicked, Direction::East, 0.5);
    let sessions = Arc::clone(&harness.state.sessions);
    let world = Arc::clone(&harness.state.world);
    let mut writer = Vec::new();
    let four = std::num::NonZeroUsize::new(4).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(four, four);
    let publisher = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(boundary.clone()),
        ScriptPlayerId::new(9),
        "123e4567-e89b-12d3-a456-426614174000",
        "PlacementAdapter",
        CommandPermissions::from_op(true),
        "minecraft:overworld",
    );
    let mut request = Box::pin(handle_block_item_placement(
        &mut harness.state,
        &mut writer,
        Some(&publisher),
        GameMode::Survival,
        harness.pose,
        clicked,
        &action,
        (clicked.x, clicked.y, clicked.z),
    ));

    poll_placement_pending(request.as_mut()).await;
    world
        .lock()
        .await
        .set_block_at(stale_neighbor, BlockStateId(1))
        .unwrap();
    assert_eq!(
        harness
            .owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    request.await.unwrap();

    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_held_count(&harness, 2);
    boundary
        .enqueue_required_event(ScriptEvent::server_tick(99))
        .await
        .unwrap();
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerTick { tick: 99 }
    ));
}

#[tokio::test]
async fn ordinary_non_stair_placement_recomputes_adjacent_stair_and_debits_once() {
    let mut harness = placement_harness(STONE_ITEM_ID).await;
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { x: 5, ..clicked };
    let north = BlockPos { z: 3, ..target };
    let existing = block_state(
        &harness.state.blocks,
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "inner_left"),
            ("waterlogged", "true"),
        ],
    );
    let expected = block_state(
        &harness.state.blocks,
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "true"),
        ],
    );
    set_block(&harness.state, clicked, BlockStateId(1)).await;
    set_block(&harness.state, north, existing).await;
    let action = use_item_on(clicked, Direction::East, 0.5);

    run_accepted_placement(&mut harness, clicked, &action).await;

    assert_eq!(
        harness.state.world.lock().await.get_cached_block(target),
        Some(BlockStateId(1))
    );
    assert_eq!(
        harness.state.world.lock().await.get_cached_block(north),
        Some(expected)
    );
    assert_held_count(&harness, 1);
}

#[tokio::test]
async fn adjacent_same_slab_merges_after_clicking_another_block() {
    let mut harness = placement_harness(SLAB_ITEM_ID).await;
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { y: 65, ..clicked };
    let bottom = block_state(
        &harness.state.blocks,
        "minecraft:oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let double = block_state(
        &harness.state.blocks,
        "minecraft:oak_slab",
        &[("type", "double"), ("waterlogged", "false")],
    );
    set_block(&harness.state, clicked, BlockStateId(1)).await;
    set_block(&harness.state, target, bottom).await;
    let action = use_item_on(clicked, Direction::Up, 1.0);
    run_accepted_placement(&mut harness, clicked, &action).await;

    assert_eq!(
        harness.state.world.lock().await.get_cached_block(target),
        Some(double)
    );
    assert_held_count(&harness, 1);
}

#[tokio::test]
async fn non_merging_half_of_clicked_slab_can_merge_the_adjacent_slab() {
    let mut harness = placement_harness(SLAB_ITEM_ID).await;
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { x: 5, ..clicked };
    let bottom = block_state(
        &harness.state.blocks,
        "minecraft:oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let double = block_state(
        &harness.state.blocks,
        "minecraft:oak_slab",
        &[("type", "double"), ("waterlogged", "false")],
    );
    set_block(&harness.state, clicked, bottom).await;
    set_block(&harness.state, target, bottom).await;
    let action = use_item_on(clicked, Direction::East, 0.25);
    run_accepted_placement(&mut harness, clicked, &action).await;

    assert_eq!(
        harness.state.world.lock().await.get_cached_block(target),
        Some(double)
    );
    assert_held_count(&harness, 1);
}

#[tokio::test]
async fn new_slab_and_stair_are_waterlogged_only_when_target_fluid_is_water() {
    for (held_item, block_name) in [
        (SLAB_ITEM_ID, "minecraft:oak_slab"),
        (STAIR_ITEM_ID, "minecraft:oak_stairs"),
    ] {
        let mut harness = placement_harness(held_item).await;
        let clicked = BlockPos { x: 4, y: 64, z: 4 };
        let target = BlockPos { y: 65, ..clicked };
        set_block(&harness.state, clicked, BlockStateId(1)).await;
        set_block(&harness.state, target, BlockStateId(88)).await;
        let action = use_item_on(clicked, Direction::Up, 1.0);
        run_accepted_placement(&mut harness, clicked, &action).await;

        let placed = harness
            .state
            .world
            .lock()
            .await
            .get_cached_block(target)
            .unwrap();
        let state = harness.state.blocks.by_id(placed).unwrap();
        assert_eq!(state.block.id.as_str(), block_name);
        assert_eq!(
            state
                .properties
                .iter()
                .find_map(|(name, value)| { (name == "waterlogged").then_some(value.as_str()) }),
            Some("true")
        );
        assert_held_count(&harness, 1);
    }
}

#[tokio::test]
async fn lava_target_rejects_slab_placement_without_inventory_debit() {
    let mut harness = placement_harness(SLAB_ITEM_ID).await;
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    let target = BlockPos { y: 65, ..clicked };
    set_block(&harness.state, clicked, BlockStateId(1)).await;
    set_block(&harness.state, target, BlockStateId(89)).await;
    let action = use_item_on(clicked, Direction::Up, 1.0);
    let sessions = Arc::clone(&harness.state.sessions);
    let world = Arc::clone(&harness.state.world);
    let mut writer = Vec::new();

    handle_block_item_placement(
        &mut harness.state,
        &mut writer,
        None,
        GameMode::Survival,
        harness.pose,
        clicked,
        &action,
        (clicked.x, clicked.y, clicked.z),
    )
    .await
    .unwrap();

    assert_eq!(
        harness
            .owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        0
    );
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(89))
    );
    assert_held_count(&harness, 2);
}

async fn run_accepted_placement(
    harness: &mut PlacementHarness,
    clicked: BlockPos,
    action: &ServerboundUseItemOn,
) {
    let sessions = Arc::clone(&harness.state.sessions);
    let world = Arc::clone(&harness.state.world);
    let mut writer = Vec::new();
    let mut request = Box::pin(handle_block_item_placement(
        &mut harness.state,
        &mut writer,
        None,
        GameMode::Survival,
        harness.pose,
        clicked,
        action,
        (clicked.x, clicked.y, clicked.z),
    ));
    poll_placement_pending(request.as_mut()).await;
    assert_eq!(
        harness
            .owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    request.await.unwrap();
}

fn placement_reports() -> Vec<BlockReport> {
    let slab = BlockReport {
        id: Identifier::parse("minecraft:oak_slab").unwrap(),
        properties: prop_schema(&[
            ("type", &["bottom", "top", "double"]),
            ("waterlogged", &["false", "true"]),
        ]),
        states: ["bottom", "top", "double"]
            .into_iter()
            .flat_map(|slab_type| {
                ["false", "true"].map(move |waterlogged| (slab_type, waterlogged))
            })
            .enumerate()
            .map(|(offset, (slab_type, waterlogged))| {
                state(
                    2 + offset as u32,
                    slab_type == "bottom" && waterlogged == "false",
                    &[("type", slab_type), ("waterlogged", waterlogged)],
                )
            })
            .collect(),
    };
    let mut stair_states = Vec::new();
    let mut id = 8;
    for facing in ["north", "south", "west", "east"] {
        for half in ["bottom", "top"] {
            for shape in [
                "straight",
                "inner_left",
                "inner_right",
                "outer_left",
                "outer_right",
            ] {
                for waterlogged in ["false", "true"] {
                    stair_states.push(state(
                        id,
                        facing == "north"
                            && half == "bottom"
                            && shape == "straight"
                            && waterlogged == "false",
                        &[
                            ("facing", facing),
                            ("half", half),
                            ("shape", shape),
                            ("waterlogged", waterlogged),
                        ],
                    ));
                    id += 1;
                }
            }
        }
    }
    vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stone"),
        slab,
        BlockReport {
            id: Identifier::parse("minecraft:oak_stairs").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north", "south", "west", "east"]),
                ("half", &["top", "bottom"]),
                (
                    "shape",
                    &[
                        "straight",
                        "inner_left",
                        "inner_right",
                        "outer_left",
                        "outer_right",
                    ],
                ),
                ("waterlogged", &["true", "false"]),
            ]),
            states: stair_states,
        },
        simple_block(88, "minecraft:water"),
        simple_block(89, "minecraft:lava"),
    ]
}

fn simple_block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(id, true, &[])],
    }
}

fn state(id: u32, default: bool, properties: &[(&str, &str)]) -> BlockStateReport {
    BlockStateReport {
        id,
        default,
        properties: properties
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

fn prop_schema(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(name, values)| {
            (
                (*name).to_owned(),
                values.iter().map(|value| (*value).to_owned()).collect(),
            )
        })
        .collect()
}
