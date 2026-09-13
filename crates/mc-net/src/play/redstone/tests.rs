//! Focused, deterministic redstone/piston tests.

use std::sync::Arc;

use mc_data::Identifier;
use mc_world::{BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};

use super::super::super::{AppliedBlockEdit, BlockEdit, block_state_property};
use super::blocks::{dust_power, role};
use super::piston::MAX_PUSH_DISTANCE;
use super::{
    MAX_POSITIONS_PER_SETTLE, MAX_POSITIONS_PER_TICK, MAX_TICKS_PER_COMMIT, RedstoneBudget, Role,
    extend_power_change_edits, metrics, redstone_tick_edits, schedule_redstone_ticks_near_applied,
};
use crate::play::block_wire::send_block_deltas;
use mc_protocol::Packet;
use mc_protocol::frame::Compression;
use mc_protocol::frame::try_decode_frame;
use mc_protocol::packets::play::{SectionBlocksUpdate, pack_section_relative_pos};

fn real_blocks() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded required block registry"),
    )
}

fn test_world(blocks: Arc<BlockRegistry>) -> WorldStorage {
    let mut world = WorldStorage::in_memory_with_capacity(blocks, 64);
    let chunk = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            chunk,
            Chunk::empty(
                chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").expect("plains biome"),
            ),
        )
        .expect("insert test chunk");
    world
}

/// Resolve a state by starting from the block default and overriding props.
fn state_id(blocks: &BlockRegistry, name: &str, overrides: &[(&str, &str)]) -> BlockStateId {
    let id = Identifier::parse(name).expect("static block identifier");
    let block = blocks.block(&id).expect("block is registered");
    let mut properties = blocks
        .by_id(block.default)
        .expect("default state")
        .properties
        .clone();
    for (key, value) in overrides {
        let slot = properties
            .iter_mut()
            .find(|(name, _)| name == key)
            .unwrap_or_else(|| panic!("{name} has no {key} property"));
        *slot = (key.to_string(), value.to_string());
    }
    blocks
        .by_name_and_props(&id, &properties)
        .unwrap_or_else(|| panic!("{name} state {overrides:?} is registered"))
}

/// Residency is part of the world contract: reads outside loaded chunks return
/// `None`, so wide fixtures must load the chunks they span.
fn load_chunk_columns(world: &mut WorldStorage, chunk_x: i32, chunk_z: i32) {
    for x in 0..=chunk_x {
        for z in 0..=chunk_z {
            let chunk = ChunkPos { x, z };
            world
                .insert_generated_chunk(
                    chunk,
                    Chunk::empty(
                        chunk,
                        BlockStateId(0),
                        Identifier::parse("minecraft:plains").expect("plains biome"),
                    ),
                )
                .expect("insert test chunk");
        }
    }
}

fn place(world: &mut WorldStorage, pos: BlockPos, state: BlockStateId) {
    world.set_block_at(pos, state).expect("place test block");
}

fn pos(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos { x, y, z }
}

fn apply_edits(world: &mut WorldStorage, edits: &[BlockEdit]) {
    for edit in edits {
        place(world, edit.pos, edit.new_state);
    }
}

fn dust_at(world: &WorldStorage, blocks: &BlockRegistry, at: BlockPos) -> u8 {
    let state_id = world
        .get_cached_block(at)
        .expect("dust position is resident");
    let state = blocks.by_id(state_id).expect("dust state is registered");
    assert!(matches!(role(state), Role::Dust), "{at:?} is not dust");
    dust_power(state)
}

fn tick_edits(world: &WorldStorage, blocks: &BlockRegistry, at: BlockPos) -> Vec<BlockEdit> {
    let state_id = world
        .get_cached_block(at)
        .expect("ticked block is resident");
    let mut budget = RedstoneBudget::new(MAX_POSITIONS_PER_SETTLE);
    redstone_tick_edits(blocks, world, at, state_id, None, &mut budget)
        .expect("component should be reactive")
}

fn control_change(
    world: &WorldStorage,
    blocks: &BlockRegistry,
    source: BlockPos,
    powered: bool,
    source_state: BlockStateId,
) -> Vec<BlockEdit> {
    let mut edits = vec![BlockEdit {
        pos: source,
        new_state: source_state,
    }];
    extend_power_change_edits(blocks, world, source, powered, None, &mut edits);
    edits
}

#[test]
fn lever_powers_a_dust_line_with_one_level_of_attenuation_per_block() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    load_chunk_columns(&mut world, 2, 0);
    let lever = pos(0, 64, 0);
    place(
        &mut world,
        lever,
        state_id(&blocks, "minecraft:lever", &[("powered", "false")]),
    );
    let dust: Vec<BlockPos> = (1..=16).map(|x| pos(x, 64, 0)).collect();
    for at in &dust {
        place(
            &mut world,
            *at,
            state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
        );
    }

    let edits = control_change(
        &world,
        &blocks,
        lever,
        true,
        state_id(&blocks, "minecraft:lever", &[("powered", "true")]),
    );
    assert!(edits.iter().any(|edit| edit.pos == lever));
    apply_edits(&mut world, &edits);

    let powers: Vec<u8> = dust
        .iter()
        .map(|at| dust_at(&world, &blocks, *at))
        .collect();
    let expected: Vec<u8> = (1..=15).rev().chain(std::iter::once(0)).collect();
    assert_eq!(powers, expected, "power must fall by one per dust block");
}

#[test]
fn sources_power_adjacent_dust() {
    let blocks = real_blocks();
    let dust = pos(1, 64, 0);
    let dust_state = state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]);

    for (block, props) in [
        ("minecraft:redstone_block", Vec::new()),
        ("minecraft:stone_button", vec![("powered", "true")]),
        ("minecraft:stone_pressure_plate", vec![("powered", "true")]),
        ("minecraft:redstone_torch", vec![("lit", "true")]),
    ] {
        let mut world = test_world(Arc::clone(&blocks));
        let source = pos(0, 64, 0);
        place(&mut world, source, state_id(&blocks, block, &props));
        place(&mut world, dust, dust_state);
        let edits = tick_edits(&world, &blocks, dust);
        apply_edits(&mut world, &edits);
        assert_eq!(
            dust_at(&world, &blocks, dust),
            15,
            "{block} must power dust"
        );
    }
}

#[test]
fn unpowered_sources_leave_dust_dark() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    place(
        &mut world,
        pos(0, 64, 0),
        state_id(&blocks, "minecraft:lever", &[("powered", "false")]),
    );
    place(
        &mut world,
        pos(1, 64, 0),
        state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
    );
    let edits = tick_edits(&world, &blocks, pos(1, 64, 0));
    apply_edits(&mut world, &edits);
    assert_eq!(dust_at(&world, &blocks, pos(1, 64, 0)), 0);
}

#[test]
fn powered_torch_support_turns_the_torch_off() {
    let blocks = real_blocks();
    let torch = pos(0, 66, 0);
    let support = pos(0, 65, 0);
    let control = pos(0, 64, 0);
    let dust = pos(1, 66, 0);

    let mut world = test_world(Arc::clone(&blocks));
    place(
        &mut world,
        torch,
        state_id(&blocks, "minecraft:redstone_torch", &[]),
    );
    place(
        &mut world,
        support,
        state_id(&blocks, "minecraft:stone", &[]),
    );
    place(
        &mut world,
        dust,
        state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
    );
    place(
        &mut world,
        control,
        state_id(&blocks, "minecraft:lever", &[("powered", "false")]),
    );

    let edits = tick_edits(&world, &blocks, torch);
    apply_edits(&mut world, &edits);
    let torch_state = blocks
        .by_id(world.get_cached_block(torch).expect("torch is resident"))
        .expect("torch state");
    assert_eq!(
        block_state_property(torch_state, "lit"),
        Some("true"),
        "an unpowered support keeps the torch lit"
    );
    assert_eq!(dust_at(&world, &blocks, dust), 15);

    place(
        &mut world,
        control,
        state_id(&blocks, "minecraft:lever", &[("powered", "true")]),
    );
    let edits = tick_edits(&world, &blocks, torch);
    apply_edits(&mut world, &edits);
    let torch_state = blocks
        .by_id(world.get_cached_block(torch).expect("torch is resident"))
        .expect("torch state");
    assert_eq!(
        block_state_property(torch_state, "lit"),
        Some("false"),
        "a powered support must switch the torch off"
    );
    assert_eq!(dust_at(&world, &blocks, dust), 0);
}

#[test]
fn dust_powers_a_remote_piston() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    let lever = pos(0, 64, 0);
    let dust = pos(1, 64, 0);
    let piston = pos(2, 64, 0);
    place(
        &mut world,
        lever,
        state_id(&blocks, "minecraft:lever", &[("powered", "false")]),
    );
    place(
        &mut world,
        dust,
        state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
    );
    place(
        &mut world,
        piston,
        state_id(
            &blocks,
            "minecraft:piston",
            &[("facing", "east"), ("extended", "false")],
        ),
    );

    let edits = control_change(
        &world,
        &blocks,
        lever,
        true,
        state_id(&blocks, "minecraft:lever", &[("powered", "true")]),
    );
    assert!(
        edits.iter().any(|edit| edit.pos == piston
            && edit.new_state
                == state_id(
                    &blocks,
                    "minecraft:piston",
                    &[("facing", "east"), ("extended", "true")]
                )),
        "dust power must extend a piston that no source touches"
    );
    assert!(
        edits.iter().any(|edit| edit.pos == pos(3, 64, 0)),
        "the head must occupy the block in front of the piston"
    );
    apply_edits(&mut world, &edits);
    assert_eq!(dust_at(&world, &blocks, dust), 15);
}

fn pushing_piston_fixture(chain: &[&str], sticky: bool) -> (Arc<BlockRegistry>, WorldStorage) {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    let piston = state_id(
        &blocks,
        if sticky {
            "minecraft:sticky_piston"
        } else {
            "minecraft:piston"
        },
        &[("facing", "east"), ("extended", "false")],
    );
    place(&mut world, pos(0, 64, 0), piston);
    place(
        &mut world,
        pos(0, 65, 0),
        state_id(&blocks, "minecraft:lever", &[("powered", "true")]),
    );
    for (index, block) in chain.iter().enumerate() {
        let block = *block;
        place(
            &mut world,
            pos(index as i32 + 1, 64, 0),
            state_id(&blocks, block, &[]),
        );
    }
    (blocks, world)
}

#[test]
fn piston_pushes_at_most_twelve_blocks() {
    let chain = vec!["minecraft:stone"; MAX_PUSH_DISTANCE];
    let (blocks, world) = pushing_piston_fixture(&chain, false);
    let edits = tick_edits(&world, &blocks, pos(0, 64, 0));
    assert!(
        edits
            .iter()
            .any(|edit| edit.pos == pos(MAX_PUSH_DISTANCE as i32 + 1, 64, 0)),
        "the furthest pushed block must land one block further"
    );
    assert!(
        edits.iter().any(|edit| edit.pos == pos(1, 64, 0)
            && block_state_property(blocks.by_id(edit.new_state).expect("head state"), "type")
                == Some("normal")),
        "the head must take the arm position"
    );
}

#[test]
fn piston_refuses_thirteen_blocks() {
    let chain = vec!["minecraft:stone"; MAX_PUSH_DISTANCE + 1];
    let (blocks, world) = pushing_piston_fixture(&chain, false);
    let edits = tick_edits(&world, &blocks, pos(0, 64, 0));
    assert!(
        edits.is_empty(),
        "a thirteen block push must be refused atomically, got {edits:?}"
    );
}

#[test]
fn piston_refuses_immovable_blocks() {
    for immovable in [
        "minecraft:obsidian",
        "minecraft:bedrock",
        "minecraft:reinforced_deepslate",
    ] {
        let (blocks, world) = pushing_piston_fixture(&["minecraft:stone", immovable], false);
        let edits = tick_edits(&world, &blocks, pos(0, 64, 0));
        assert!(
            edits.is_empty(),
            "{immovable} must refuse the whole move, got {edits:?}"
        );
    }
}

#[test]
fn piston_refuses_block_entity_blocks_without_losing_contents() {
    let (blocks, world) = pushing_piston_fixture(&["minecraft:chest"], false);
    let chest = pos(1, 64, 0);
    let edits = tick_edits(&world, &blocks, pos(0, 64, 0));
    assert!(
        edits.is_empty(),
        "a chest must refuse the move rather than desync its contents, got {edits:?}"
    );
    // Nothing was planned, so the chest and the piston are untouched and no
    // block exists at the would-be destination.
    let chest_state = world.get_cached_block(chest).expect("chest is resident");
    assert_eq!(
        blocks
            .by_id(chest_state)
            .expect("chest state")
            .block
            .id
            .path(),
        "chest"
    );
    assert_eq!(
        world.get_cached_block(pos(2, 64, 0)),
        Some(state_id(&blocks, "minecraft:air", &[]))
    );
    let piston_state = world
        .get_cached_block(pos(0, 64, 0))
        .expect("piston is resident");
    assert_eq!(
        block_state_property(
            blocks.by_id(piston_state).expect("piston state"),
            "extended"
        ),
        Some("false"),
        "a refused move leaves the piston retracted"
    );
}

#[test]
fn sticky_piston_pulls_the_nearest_block_back() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    // Extended sticky piston: base, head, then two pushed stone blocks.
    place(
        &mut world,
        pos(0, 64, 0),
        state_id(
            &blocks,
            "minecraft:sticky_piston",
            &[("facing", "east"), ("extended", "true")],
        ),
    );
    place(
        &mut world,
        pos(1, 64, 0),
        state_id(
            &blocks,
            "minecraft:piston_head",
            &[("facing", "east"), ("short", "false"), ("type", "sticky")],
        ),
    );
    place(
        &mut world,
        pos(2, 64, 0),
        state_id(&blocks, "minecraft:stone", &[]),
    );
    place(
        &mut world,
        pos(3, 64, 0),
        state_id(&blocks, "minecraft:stone", &[]),
    );

    let edits = tick_edits(&world, &blocks, pos(0, 64, 0));
    assert!(
        edits.iter().any(|edit| edit.pos == pos(1, 64, 0)
            && edit.new_state == state_id(&blocks, "minecraft:stone", &[])),
        "the sticky head must pull the nearest block into the head position"
    );
    assert!(
        edits.iter().any(|edit| edit.pos == pos(2, 64, 0)
            && edit.new_state == state_id(&blocks, "minecraft:air", &[])),
        "the pulled block's old position must become air"
    );
    apply_edits(&mut world, &edits);
    assert_eq!(
        world.get_cached_block(pos(0, 64, 0)),
        Some(state_id(
            &blocks,
            "minecraft:sticky_piston",
            &[("facing", "east"), ("extended", "false")]
        ))
    );
    assert_eq!(
        world.get_cached_block(pos(3, 64, 0)),
        Some(state_id(&blocks, "minecraft:stone", &[])),
        "blocks behind the pulled one stay put"
    );
}

#[test]
fn plain_piston_does_not_pull() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    place(
        &mut world,
        pos(0, 64, 0),
        state_id(
            &blocks,
            "minecraft:piston",
            &[("facing", "east"), ("extended", "true")],
        ),
    );
    place(
        &mut world,
        pos(1, 64, 0),
        state_id(
            &blocks,
            "minecraft:piston_head",
            &[("facing", "east"), ("short", "false"), ("type", "normal")],
        ),
    );
    place(
        &mut world,
        pos(2, 64, 0),
        state_id(&blocks, "minecraft:stone", &[]),
    );

    let edits = tick_edits(&world, &blocks, pos(0, 64, 0));
    apply_edits(&mut world, &edits);
    assert_eq!(
        world.get_cached_block(pos(1, 64, 0)),
        Some(state_id(&blocks, "minecraft:air", &[]))
    );
    assert_eq!(
        world.get_cached_block(pos(2, 64, 0)),
        Some(state_id(&blocks, "minecraft:stone", &[])),
        "a plain piston leaves the block behind"
    );
}

#[test]
fn dust_settle_stops_at_the_budget_fence() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    load_chunk_columns(&mut world, 25, 0);
    let lever = pos(0, 64, 0);
    place(
        &mut world,
        lever,
        state_id(&blocks, "minecraft:lever", &[("powered", "false")]),
    );
    let line: Vec<BlockPos> = (1..=400).map(|x| pos(x, 64, 0)).collect();
    for at in &line {
        place(
            &mut world,
            *at,
            state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
        );
    }

    let before = metrics().budget_drops;
    let edits = control_change(
        &world,
        &blocks,
        lever,
        true,
        state_id(&blocks, "minecraft:lever", &[("powered", "true")]),
    );
    assert!(
        edits.len() <= MAX_POSITIONS_PER_SETTLE + 1,
        "a pathological network must not plan more than the fence allows: {}",
        edits.len()
    );
    assert!(
        metrics().budget_drops > before,
        "hitting the fence must be observable"
    );
    apply_edits(&mut world, &edits);
    assert_eq!(dust_at(&world, &blocks, line[0]), 15);
    assert_eq!(
        dust_at(&world, &blocks, *line.last().expect("last dust")),
        0,
        "work past the fence is dropped, not deferred"
    );
}

#[test]
fn commit_hook_enqueues_bounded_and_deduplicated_ticks() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    let dust = pos(1, 64, 0);
    place(
        &mut world,
        dust,
        state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
    );
    let applied = vec![AppliedBlockEdit {
        pos: pos(0, 64, 0),
        previous: state_id(&blocks, "minecraft:air", &[]),
        new_state: state_id(&blocks, "minecraft:lever", &[("powered", "true")]),
    }];
    assert_eq!(
        schedule_redstone_ticks_near_applied(&mut world, 5, &applied),
        1
    );
    let ticks = world
        .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
        .expect("read scheduled ticks")
        .expect("resident chunk exposes ticks");
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, dust);
    assert_eq!(ticks[0].trigger_tick, 6);
    assert_eq!(
        schedule_redstone_ticks_near_applied(&mut world, 5, &applied),
        0,
        "an already queued tick must not be duplicated"
    );

    // A burst of edits is capped, and the drop is observable.
    let mut burst_world = test_world(Arc::clone(&blocks));
    load_chunk_columns(&mut burst_world, 8, 0);
    let mut burst = Vec::new();
    for x in 1..=(MAX_TICKS_PER_COMMIT as i32 * 2) {
        let at = pos(x, 100, 0);
        place(
            &mut burst_world,
            at,
            state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
        );
        burst.push(AppliedBlockEdit {
            pos: at,
            previous: state_id(&blocks, "minecraft:air", &[]),
            new_state: state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
        });
    }
    let before = metrics().ticks_dropped;
    assert_eq!(
        schedule_redstone_ticks_near_applied(&mut burst_world, 5, &burst),
        MAX_TICKS_PER_COMMIT
    );
    assert!(metrics().ticks_dropped > before);
}

#[test]
fn scheduled_dust_tick_settles_the_network_end_to_end() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    let lever = pos(0, 64, 0);
    let dust = pos(1, 64, 0);
    let powered_lever = state_id(&blocks, "minecraft:lever", &[("powered", "true")]);
    place(&mut world, lever, powered_lever);
    place(
        &mut world,
        dust,
        state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
    );

    // A committed edit (here a placement) enqueues the neighbouring component.
    let applied = vec![AppliedBlockEdit {
        pos: lever,
        previous: state_id(&blocks, "minecraft:air", &[]),
        new_state: powered_lever,
    }];
    assert_eq!(
        schedule_redstone_ticks_near_applied(&mut world, 10, &applied),
        1
    );

    let chunk = ChunkPos { x: 0, z: 0 };
    let read = world.read_view();
    let snapshot = read.snapshot_chunks(&[chunk]);
    let due = super::super::super::due_scheduled_block_ticks(&snapshot, &[chunk], 11, 16);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].pos, dust);

    let dust_state = world.get_cached_block(dust).expect("dust is resident");
    let edits = super::super::super::scheduled_block_tick_edits(
        &blocks, &mut world, due[0].pos, dust_state,
    )
    .expect("dust has a redstone tick behaviour");
    apply_edits(&mut world, &edits);
    assert_eq!(dust_at(&world, &blocks, dust), 15);
}

#[test]
fn tick_batch_budget_is_shared_across_the_batch() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    load_chunk_columns(&mut world, 16, 0);
    let line_len = (MAX_POSITIONS_PER_TICK / 4) as i32;
    let mut heads = Vec::new();
    for line in 0..5 {
        let z = line * 2;
        place(
            &mut world,
            pos(0, 64, z),
            state_id(&blocks, "minecraft:lever", &[("powered", "true")]),
        );
        for x in 1..=line_len {
            place(
                &mut world,
                pos(x, 64, z),
                state_id(&blocks, "minecraft:redstone_wire", &[("power", "0")]),
            );
        }
        heads.push(pos(1, 64, z));
    }
    // One deliberately stale late line: it must stay untouched once the batch
    // budget is spent.
    let stale_line = 4;
    let stale_tail = pos(line_len, 64, stale_line * 2);
    place(
        &mut world,
        stale_tail,
        state_id(&blocks, "minecraft:redstone_wire", &[("power", "15")]),
    );

    let before = metrics().budget_drops;
    let mut budget = RedstoneBudget::new(MAX_POSITIONS_PER_TICK);
    for head in &heads {
        let state_id = world.get_cached_block(*head).expect("dust is resident");
        let edits = redstone_tick_edits(&blocks, &world, *head, state_id, None, &mut budget)
            .expect("dust has a redstone tick behaviour");
        apply_edits(&mut world, &edits);
    }

    assert!(
        metrics().budget_drops > before,
        "a batch larger than the per-tick fence must record the drop"
    );
    assert_eq!(dust_at(&world, &blocks, heads[0]), 15);
    assert_eq!(
        dust_at(&world, &blocks, stale_tail),
        15,
        "the line past the spent batch budget keeps its stale state"
    );
}

#[test]
fn non_reactive_blocks_keep_their_existing_tick_behaviour() {
    let blocks = real_blocks();
    let mut world = test_world(Arc::clone(&blocks));
    let button = pos(1, 64, 0);
    place(
        &mut world,
        button,
        state_id(&blocks, "minecraft:stone_button", &[("powered", "true")]),
    );
    let state_id_powered = world.get_cached_block(button).expect("button is resident");
    let mut budget = RedstoneBudget::new(MAX_POSITIONS_PER_SETTLE);
    assert!(
        redstone_tick_edits(&blocks, &world, button, state_id_powered, None, &mut budget).is_none(),
        "buttons keep the existing scheduled-tick release path"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn piston_extension_reaches_clients_as_block_updates() {
    let (blocks, mut world) = pushing_piston_fixture(&["minecraft:stone"], true);
    let edits = tick_edits(&world, &blocks, pos(0, 64, 0));
    assert!(!edits.is_empty());
    let outcome =
        super::super::super::block_edit_commit::apply_block_edit_batch_to_storage_conditionally(
            &mut world,
            None,
            &edits,
            &[],
        )
        .expect("piston edits commit against the unchanged world");

    let mut wire = Vec::new();
    send_block_deltas(&mut wire, Compression::Disabled, &outcome.deltas, None)
        .await
        .expect("encode piston deltas");
    let mut frames = bytes::BytesMut::from(wire.as_slice());
    let mut frame = try_decode_frame(&mut frames, Compression::Disabled)
        .expect("decode piston frame")
        .expect("piston frame");
    assert_eq!(frame.id, SectionBlocksUpdate::ID);
    let update = SectionBlocksUpdate::decode(&mut frame.body).expect("decode section update");
    let head = state_id(
        &blocks,
        "minecraft:piston_head",
        &[("facing", "east"), ("short", "false"), ("type", "sticky")],
    );
    let base = state_id(
        &blocks,
        "minecraft:sticky_piston",
        &[("facing", "east"), ("extended", "true")],
    );
    let retracted_base = state_id(
        &blocks,
        "minecraft:sticky_piston",
        &[("facing", "east"), ("extended", "false")],
    );
    let air = state_id(&blocks, "minecraft:air", &[]);
    let stone = state_id(&blocks, "minecraft:stone", &[]);
    let changed: Vec<(u16, i32)> = update
        .changes
        .iter()
        .map(|change| (change.relative_pos, change.state_id))
        .collect();
    assert!(
        changed.contains(&(pack_section_relative_pos(0, 64, 0), base.0 as i32)),
        "the client must see the extended base: {changed:?}"
    );
    assert!(
        changed.contains(&(pack_section_relative_pos(1, 64, 0), head.0 as i32)),
        "the client must see the head: {changed:?}"
    );

    // Retraction must be visible the same way.
    place(
        &mut world,
        pos(0, 65, 0),
        state_id(&blocks, "minecraft:lever", &[("powered", "false")]),
    );
    let retract = tick_edits(&world, &blocks, pos(0, 64, 0));
    assert!(!retract.is_empty());
    let outcome =
        super::super::super::block_edit_commit::apply_block_edit_batch_to_storage_conditionally(
            &mut world,
            None,
            &retract,
            &[],
        )
        .expect("retract edits commit against the unchanged world");
    let mut wire = Vec::new();
    send_block_deltas(&mut wire, Compression::Disabled, &outcome.deltas, None)
        .await
        .expect("encode retract deltas");
    let mut frames = bytes::BytesMut::from(wire.as_slice());
    let mut retracted = Vec::new();
    while let Some(mut frame) =
        try_decode_frame(&mut frames, Compression::Disabled).expect("decode retract frame")
    {
        if frame.id == SectionBlocksUpdate::ID {
            retracted.extend(
                SectionBlocksUpdate::decode(&mut frame.body)
                    .expect("decode retract section update")
                    .changes
                    .into_iter()
                    .map(|change| (change.relative_pos, change.state_id)),
            );
        }
    }
    assert!(
        retracted.contains(&(pack_section_relative_pos(0, 64, 0), retracted_base.0 as i32)),
        "the client must see the retracted base: {retracted:?}"
    );
    assert!(
        retracted.contains(&(pack_section_relative_pos(1, 64, 0), stone.0 as i32)),
        "the client must see the sticky head replaced by the pulled block: {retracted:?}"
    );
    assert!(
        retracted.contains(&(pack_section_relative_pos(2, 64, 0), air.0 as i32)),
        "the client must see the pulled block's old position cleared: {retracted:?}"
    );
}
