use super::*;
use crate::world::ChunkWorld;
use std::sync::{Arc, LazyLock};

const HOPPER: BlockPos = BlockPos { x: 3, y: 201, z: 3 };

fn state() -> TickState {
    state_with_dir(None, true)
}

fn state_with_dir(dir: Option<std::path::PathBuf>, clear: bool) -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let air = mc_world::BlockStateId(registry.states.air);
    let mut world = if let Some(dir) = dir {
        let config = crate::CoreConfig::new(dir, 42);
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&config).unwrap(),
            crate::native_checkpoint::dimension_id(crate::dimensions::Dimension::Overworld),
            generator,
            &registry,
            air,
            [3, 201, 3],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            air,
            [3, 201, 3],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("containers-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    if clear {
        for x in 0..8 {
            for y in 200..205 {
                for z in 0..8 {
                    state.world.set_block(x, y, z, state.registry.states.air);
                }
            }
        }
    }
    state.world.fold_overlay();
    state
}

fn place(state: &mut TickState, pos: BlockPos, name: &str, props: &[(&str, &str)]) {
    let id = Identifier::parse(format!("minecraft:{name}")).unwrap();
    let block = state.registry.blocks.block(&id).unwrap();
    let mut properties = state
        .registry
        .blocks
        .by_id(block.default)
        .unwrap()
        .properties
        .clone();
    for (key, value) in props {
        properties
            .iter_mut()
            .find(|(name, _)| name == key)
            .unwrap()
            .1 = (*value).into();
    }
    let block = state
        .registry
        .blocks
        .by_name_and_props(&id, &properties)
        .unwrap()
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, block);
    state.world.fold_overlay();
    register(state, pos);
}

fn item(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(item_id(state, name).unwrap(), count)
}

fn set_slot(state: &mut TickState, pos: BlockPos, index: usize, stack: ItemStack) {
    state.block_entities.get_mut(&key(pos)).unwrap().slots[index] = stack;
    inventory_changed(&mut state.redstone_containers, pos);
}

fn step(state: &mut TickState) {
    state.tick += 1;
    run(state, &mut Vec::new(), &mut Vec::new());
}

fn total(state: &TickState) -> i32 {
    let inventories: i32 = state
        .block_entities
        .values()
        .flat_map(|record| &record.slots)
        .map(|stack| stack.count.max(0))
        .sum();
    inventories
        + state
            .entities
            .slots()
            .filter(|slot| !state.merge_consumed_items.contains(slot))
            .filter_map(|slot| state.entities.row(slot).item.as_ref())
            .map(|stack| stack.count)
            .sum::<i32>()
}

#[test]
fn comparator_reads_item_frames_through_backing_block_in_all_horizontal_directions() {
    use super::super::{offset, signal::comparator_input};
    use crate::boundary::EntityDataValue;

    let mut state = state();
    place(&mut state, HOPPER, "stone", &[]);
    for (direction, face) in [
        ((1, 0, 0), 5),
        ((-1, 0, 0), 4),
        ((0, 0, 1), 3),
        ((0, 0, -1), 2),
    ] {
        let comparator = offset(HOPPER, (-direction.0, 0, -direction.2)).unwrap();
        let position = offset(HOPPER, direction).unwrap();
        for glow in [false, true] {
            let empty =
                crate::item_frame::spawn(&mut state, position, face, glow, ItemStack::EMPTY)
                    .unwrap();
            assert_eq!(comparator_input(&state, comparator, direction), 0);
            state.remove_entity(state.entities.slot_of(empty).unwrap());

            let stack = item(&state, "diamond", 1);
            let frame = crate::item_frame::spawn(&mut state, position, face, glow, stack).unwrap();
            let slot = state.entities.slot_of(frame).unwrap();
            for rotation in 0..8 {
                let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
                appearance.extra.retain(|(index, _)| *index != 10);
                appearance
                    .extra
                    .push((10, EntityDataValue::VarInt(rotation)));
                assert_eq!(
                    comparator_input(&state, comparator, direction),
                    rotation as u8 + 1,
                );
            }
            Arc::make_mut(&mut state.entities.row_mut(slot).appearance).spawn_data =
                i32::from(face ^ 1);
            assert_eq!(comparator_input(&state, comparator, direction), 0);
            state.remove_entity(slot);
            assert_eq!(comparator_input(&state, comparator, direction), 0);
        }
    }
}

#[test]
fn hopper_pushes_and_pulls_once_then_waits_eight_ticks() {
    let mut state = state();
    let chest = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let above = super::super::offset(HOPPER, (0, 1, 0)).unwrap();
    place(
        &mut state,
        HOPPER,
        "hopper",
        &[("facing", "east"), ("enabled", "true")],
    );
    place(&mut state, chest, "chest", &[]);
    place(&mut state, above, "barrel", &[]);
    let stone = item(&state, "stone", 3);
    set_slot(&mut state, HOPPER, 0, stone.clone());
    set_slot(&mut state, above, 0, stone);
    step(&mut state);
    assert_eq!(state.block_entities[&key(chest)].slots[0].count, 1);
    assert_eq!(state.block_entities[&key(above)].slots[0].count, 2);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 3);
    assert_eq!(
        state.redstone_containers.remaining(key(HOPPER), state.tick),
        8
    );
    for _ in 0..7 {
        step(&mut state);
    }
    assert_eq!(state.block_entities[&key(chest)].slots[0].count, 1);
    step(&mut state);
    assert_eq!(state.block_entities[&key(chest)].slots[0].count, 2);
    assert_eq!(total(&state), 6);
}

#[test]
fn disabled_hopper_keeps_custody_without_restarting_cooldown() {
    let mut state = state();
    let chest = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(
        &mut state,
        HOPPER,
        "hopper",
        &[("facing", "east"), ("enabled", "false")],
    );
    place(&mut state, chest, "chest", &[]);
    let stone = item(&state, "stone", 2);
    set_slot(&mut state, HOPPER, 0, stone);
    state
        .redstone_containers
        .set_cooldown(key(HOPPER), 3, state.tick);
    for _ in 0..5 {
        step(&mut state);
    }
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 2);
    assert!(
        state.block_entities[&key(chest)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    assert_eq!(
        state.redstone_containers.remaining(key(HOPPER), state.tick),
        0
    );
}

#[test]
fn blocked_hopper_sleeps_until_a_neighbour_inventory_changes() {
    let mut state = state();
    let chest = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "hopper", &[("facing", "east")]);
    place(&mut state, chest, "chest", &[]);
    let stone = item(&state, "stone", 64);
    state
        .block_entities
        .get_mut(&key(chest))
        .unwrap()
        .slots
        .fill(stone.clone());
    let hopper_stone = item(&state, "stone", 2);
    set_slot(&mut state, HOPPER, 0, hopper_stone);

    step(&mut state);

    assert!(state.redstone_containers.sleeping.contains(&key(HOPPER)));
    assert_eq!(total(&state), 27 * 64 + 2);

    let chest_stone = item(&state, "stone", 63);
    set_slot(&mut state, chest, 0, chest_stone);
    assert!(!state.redstone_containers.sleeping.contains(&key(HOPPER)));
    step(&mut state);

    assert_eq!(state.block_entities[&key(chest)].slots[0].count, 64);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
    assert_eq!(total(&state), 27 * 64 + 1);
    assert_eq!(
        state.redstone_containers.remaining(key(HOPPER), state.tick),
        8
    );
}

#[test]
fn hopper_lane_tracks_loaded_hoppers_without_container_scanning() {
    let mut state = state();
    place(&mut state, HOPPER, "hopper", &[]);
    assert_eq!(
        state.redstone_containers.hoppers,
        std::collections::BTreeSet::from([key(HOPPER)])
    );

    state
        .world
        .set_block(HOPPER.x, HOPPER.y, HOPPER.z, state.registry.states.air);
    state.world.fold_overlay();
    step(&mut state);

    assert!(state.redstone_containers.hoppers.is_empty());
    assert!(state.redstone_containers.cooldowns.is_empty());
}

#[test]
fn hopper_chain_to_chest_never_duplicates_or_moves_received_item_same_tick() {
    let mut state = state();
    let next = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let chest = super::super::offset(next, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "hopper", &[("facing", "east")]);
    place(&mut state, next, "hopper", &[("facing", "east")]);
    place(&mut state, chest, "chest", &[]);
    let stone = item(&state, "stone", 16);
    set_slot(&mut state, HOPPER, 0, stone);
    step(&mut state);
    assert_eq!(state.block_entities[&key(next)].slots[0].count, 1);
    assert!(state.block_entities[&key(chest)].slots[0].is_empty());
    for _ in 0..160 {
        step(&mut state);
        assert_eq!(total(&state), 16);
    }
    assert_eq!(state.block_entities[&key(chest)].slots[0].count, 16);
    assert!(
        state.block_entities[&key(HOPPER)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    assert!(
        state.block_entities[&key(next)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
}

#[test]
fn blocked_push_and_partial_suction_preserve_components_and_counts() {
    let mut state = state();
    let chest = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "hopper", &[("facing", "east")]);
    place(&mut state, chest, "chest", &[]);
    let stone = item(&state, "stone", 64).with_custom_name("identity");
    state
        .block_entities
        .get_mut(&key(chest))
        .unwrap()
        .slots
        .fill(stone.clone());
    state
        .block_entities
        .get_mut(&key(HOPPER))
        .unwrap()
        .slots
        .fill(stone.clone());
    state.block_entities.get_mut(&key(HOPPER)).unwrap().slots[4].count = 63;
    let mut dropped = stone.clone();
    dropped.count = 10;
    let id = spawn_item_entity(&mut state, [3.5, 202.0, 3.5], dropped);
    let slot = state.entities.slot_of(id).unwrap();
    let before = total(&state);
    step(&mut state);
    assert_eq!(total(&state), before);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[4].count, 64);
    let rest = state.entities.row(slot).item.as_ref().unwrap();
    assert_eq!(rest.count, 9);
    assert_eq!(rest.full_stack().custom_name, stone.custom_name);
    assert_eq!(
        state
            .entities
            .row(slot)
            .appearance
            .item
            .as_ref()
            .unwrap()
            .count,
        9
    );
    assert!(state.appearance_changed.contains(&id));
    assert!(!state.merge_consumed_items.contains(&slot));
}

#[test]
fn full_suction_is_staged_for_commit_and_cannot_be_collected_twice() {
    let mut state = state();
    place(&mut state, HOPPER, "hopper", &[]);
    let stone = item(&state, "stone", 7).with_custom_name("keep");
    let id = spawn_item_entity(&mut state, [3.5, 202.0, 3.5], stone.clone());
    let slot = state.entities.slot_of(id).unwrap();
    step(&mut state);
    assert!(state.merge_consumed_items.contains(&slot));
    assert_eq!(total(&state), 7);
    state
        .redstone_containers
        .set_cooldown(key(HOPPER), 0, state.tick);
    step(&mut state);
    assert_eq!(total(&state), 7);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0], stone);
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(output.changes.entities_removed.contains(&id));
    assert!(state.entities.slot_of(id).is_none());
}

#[test]
fn full_hopper_leaves_bowl_item_until_inventory_has_room() {
    let mut state = state();
    place(&mut state, HOPPER, "hopper", &[]);
    let dirt = item(&state, "dirt", 64);
    state
        .block_entities
        .get_mut(&key(HOPPER))
        .unwrap()
        .slots
        .fill(dirt.clone());
    let id = spawn_item_entity(&mut state, [3.5, 201.6875, 3.5], dirt.clone());
    let slot = state.entities.slot_of(id).unwrap();
    step(&mut state);
    assert!(!state.merge_consumed_items.contains(&slot));
    assert_eq!(state.entities.row(slot).item.as_ref().unwrap().count, 64);
    assert_eq!(total(&state), 384);
    let nearly_full = item(&state, "dirt", 63);
    set_slot(&mut state, HOPPER, 4, nearly_full);
    step(&mut state);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[4].count, 64);
    assert_eq!(state.entities.row(slot).item.as_ref().unwrap().count, 63);
    assert_eq!(total(&state), 383);
    assert!(state.appearance_changed.contains(&id));
}

#[test]
fn hopper_solid_lid_prevents_entity_suction() {
    let mut state = state();
    place(&mut state, HOPPER, "hopper", &[]);
    let above = super::super::offset(HOPPER, (0, 1, 0)).unwrap();
    state
        .world
        .set_block(above.x, above.y, above.z, state.registry.states.stone);
    state.world.fold_overlay();
    let stone = item(&state, "stone", 2);
    spawn_item_entity(&mut state, [3.5, 202.0, 3.5], stone);
    step(&mut state);
    assert!(
        state.block_entities[&key(HOPPER)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    assert!(state.merge_consumed_items.is_empty());
}

#[test]
fn dropper_inserts_or_ejects_but_never_ejects_blocked_container_transfer() {
    let mut state = state();
    let chest = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dropper", &[("facing", "east")]);
    place(&mut state, chest, "chest", &[]);
    let stone = item(&state, "stone", 3).with_custom_name("source");
    set_slot(&mut state, HOPPER, 0, stone.clone());
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert!(spawned.is_empty());
    assert_eq!(state.block_entities[&key(chest)].slots[0].count, 1);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 2);
    let full = item(&state, "stone", 64);
    state
        .block_entities
        .get_mut(&key(chest))
        .unwrap()
        .slots
        .fill(full);
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert!(spawned.is_empty());
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 2);
    state
        .world
        .set_block(chest.x, chest.y, chest.z, state.registry.states.air);
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(spawned.len(), 1);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
    let entity = state.entities.slot_of(spawned[0]).unwrap();
    // Core velocities are blocks/second, while vanilla ejection is 0.2
    // blocks/tick. A missing x20 conversion strands items inside the ejector.
    assert_eq!(state.entities.vel(entity), [4.0, 4.0, 0.0]);
    let ejected = state
        .entities
        .row(entity)
        .item
        .as_ref()
        .unwrap()
        .full_stack();
    assert_eq!(ejected.count, 1);
    assert_eq!(ejected.custom_name, stone.custom_name);
}

#[test]
fn dispenser_consumes_projectile_once_and_unknown_item_ejects_once() {
    let mut state = state();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let arrow = item(&state, "arrow", 2);
    set_slot(&mut state, HOPPER, 0, arrow);
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert!(spawned.is_empty());
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).projectile.is_some())
            .count(),
        1
    );
    let stone = item(&state, "stone", 2);
    set_slot(&mut state, HOPPER, 0, stone);
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(spawned.len(), 1);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
}

#[test]
fn dispenser_bucket_actions_commit_block_and_remainder_together() {
    let mut state = state();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let bucket = item(&state, "water_bucket", 1);
    set_slot(&mut state, HOPPER, 0, bucket);
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.water_levels[0])
    );
    assert_eq!(
        writes,
        vec![(target, state.registry.states.water_levels[0])]
    );
    assert_eq!(
        state.block_entities[&key(HOPPER)].slots[0],
        item(&state, "bucket", 1)
    );
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.air)
    );
    assert_eq!(
        state.block_entities[&key(HOPPER)].slots[0],
        item(&state, "water_bucket", 1)
    );
    assert!(spawned.is_empty());
}

#[test]
fn hopper_nbt_roundtrip_keeps_remaining_cooldown_and_item_components() {
    let mut state = state();
    place(&mut state, HOPPER, "hopper", &[]);
    let stone = item(&state, "stone", 11).with_custom_name("durable");
    set_slot(&mut state, HOPPER, 3, stone.clone());
    state
        .redstone_containers
        .set_cooldown(key(HOPPER), 5, state.tick);
    checkpoint(&mut state);
    let chunk = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    let bytes = chunk.block_entities.get(&HOPPER).unwrap();
    let (decoded, cooldown) = decode(bytes, &state.registry.items, 5, state.tick).unwrap();
    assert_eq!(decoded[3], stone);
    assert_eq!(cooldown, 5);
    let mut inventories = crate::redstone::custody::BlockEntities::default();
    let mut containers = ContainerState::default();
    let decoded = decode_chunk(
        &inventories,
        &containers,
        &state.registry,
        chunk,
        state.tick,
    )
    .unwrap();
    hydrate_chunk(
        &mut inventories,
        &mut containers,
        &state.registry,
        chunk,
        state.tick,
        decoded,
    );
    assert_eq!(containers.remaining(key(HOPPER), state.tick), 5);
    assert_eq!(inventories[&key(HOPPER)].slots[3], stone);
    assert!(state.opaque_changes.iter().any(|(pos, _)| *pos == HOPPER));
    state.redstone_containers = containers;
    state.block_entities = inventories;
    for _ in 0..4 {
        step(&mut state);
    }
    assert_eq!(
        state.redstone_containers.remaining(key(HOPPER), state.tick),
        1
    );
    assert_eq!(total(&state), 11);
}

#[test]
fn chest_comparator_analog_uses_slot_fullness_and_component_stack_limit() {
    let mut state = state();
    place(&mut state, HOPPER, "chest", &[]);
    assert_eq!(signal(&state, HOPPER), 0);
    let stone = item(&state, "stone", 1);
    set_slot(&mut state, HOPPER, 0, stone);
    assert_eq!(signal(&state, HOPPER), 1);
    let stone = item(&state, "stone", 64);
    state
        .block_entities
        .get_mut(&key(HOPPER))
        .unwrap()
        .slots
        .fill(stone);
    assert_eq!(signal(&state, HOPPER), 15);
    let stone = item(&state, "stone", 32);
    state
        .block_entities
        .get_mut(&key(HOPPER))
        .unwrap()
        .slots
        .fill(stone);
    assert_eq!(signal(&state, HOPPER), 8);
}

#[test]
fn dispenser_bone_meal_grows_crop_and_failed_optional_action_keeps_item() {
    let mut state = state();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, target, "wheat", &[("age", "0")]);
    let bone = item(&state, "bone_meal", 2);
    set_slot(&mut state, HOPPER, 0, bone);
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
    assert!(!writes.is_empty());
    assert!(spawned.is_empty());
    place(&mut state, target, "wheat", &[("age", "7")]);
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
    assert!(spawned.is_empty());
}

#[test]
fn hopper_custody_and_cooldown_survive_real_native_checkpoint() {
    let dir = std::env::temp_dir().join(format!("hopper-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state_with_dir(Some(dir.clone()), true);
    place(&mut original, HOPPER, "hopper", &[]);
    let stone = item(&original, "stone", 7).with_custom_name("checkpoint");
    let id = spawn_item_entity(&mut original, [3.5, 201.6875, 3.5], stone.clone());
    step(&mut original);
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    assert!(output.changes.entities_removed.contains(&id));
    let remaining = original.redstone_containers.cooldowns[&key(HOPPER)];
    let checkpoint_tick = original.tick;
    checkpoint(&mut original);
    let capture = original.world.capture_dirty(&BTreeMap::new());
    let written = capture.write().unwrap();
    assert!(!written.is_empty());
    original.world.mark_flushed(&written);
    assert!(
        original
            .world
            .evict_outside(&Default::default())
            .contains(&ChunkPos { x: 0, z: 0 }),
        "durable native checkpoint ACK permits clean-revision eviction"
    );
    drop(original);
    let mut restored = state_with_dir(Some(dir.clone()), false);
    restored.tick = checkpoint_tick;
    let chunk = restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    let decoded = decode_chunk(
        &restored.block_entities,
        &restored.redstone_containers,
        &restored.registry,
        chunk,
        restored.tick,
    )
    .unwrap();
    hydrate_chunk(
        &mut restored.block_entities,
        &mut restored.redstone_containers,
        &restored.registry,
        chunk,
        restored.tick,
        decoded,
    );
    assert_eq!(restored.block_entities[&key(HOPPER)].slots[0], stone);
    assert_eq!(
        restored.redstone_containers.cooldowns[&key(HOPPER)],
        remaining
    );
    assert!(restored.entities.is_empty());
    assert_eq!(total(&restored), 7);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn full_suction_wal_batch_replays_inventory_cooldown_and_entity_tombstone() {
    use crate::journal::{
        EntityAppearanceImage, JournalEntry, JournalWriter, Replay, read_complete_batches_after,
        replay_batches,
    };
    let dir = std::env::temp_dir().join(format!("hopper-wal-{}", uuid::Uuid::new_v4()));
    let mut state = state();
    place(&mut state, HOPPER, "hopper", &[]);
    let stone = item(&state, "stone", 7).with_custom_name("wal");
    let id = spawn_item_entity(&mut state, [3.5, 201.6875, 3.5], stone.clone());
    let slot = state.entities.slot_of(id).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let birth = JournalEntry::Entity {
        id: id.0,
        uuid: row.uuid.to_string(),
        type_id: state.registry.item_type_id,
        health: row.health,
        alive: body.alive,
        age: row.age,
        breed_cooldown: 0,
        pickup_delay: body.pickup_delay,
        item: Some(stone.clone()),
        appearance: EntityAppearanceImage::from(row.appearance.as_ref()),
        active_effects: Default::default(),
        villager: None,
        state: Default::default(),
        projectile: None,
        tick: 1,
        pos: state.entities.pos(slot),
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
    };
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, 1, &[birth]).unwrap();
    step(&mut state);
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(output.changes.entities_removed.contains(&id));
    let remaining = state.redstone_containers.cooldowns[&key(HOPPER)];
    let mut entries: Vec<_> = state
        .opaque_changes
        .iter()
        .map(|(pos, nbt)| JournalEntry::OpaqueBlockEntity {
            pos: [pos.x, pos.y, pos.z],
            nbt: nbt.clone(),
        })
        .collect();
    entries.push(JournalEntry::BlockEntity {
        pos: [HOPPER.x, HOPPER.y, HOPPER.z],
        slots: state.block_entities[&key(HOPPER)].slots.clone(),
        furnace: None,
        tick: 2,
    });
    entries.extend(
        output
            .changes
            .entities_removed
            .iter()
            .map(|id| JournalEntry::RemoveEntity { id: id.0, tick: 2 }),
    );
    writer.append(2, 2, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    assert!(!replay.entities.contains_key(&id.0));
    assert_eq!(replay.block_entities[&key(HOPPER)].slots[0], stone);
    let bytes = replay.opaque_block_entities[&key(HOPPER)].as_ref().unwrap();
    let (slots, cooldown) = decode(bytes, &state.registry.items, 5, state.tick).unwrap();
    assert_eq!(slots[0], stone);
    assert_eq!(cooldown, remaining);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn powered_hopper_never_transfers_even_before_enabled_property_settles() {
    let mut state = state();
    let chest = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let power = super::super::offset(HOPPER, (0, -1, 0)).unwrap();
    place(
        &mut state,
        HOPPER,
        "hopper",
        &[("facing", "east"), ("enabled", "true")],
    );
    place(&mut state, chest, "chest", &[]);
    place(&mut state, power, "redstone_block", &[]);
    let stone = item(&state, "stone", 2);
    set_slot(&mut state, HOPPER, 0, stone);
    step(&mut state);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 2);
    assert!(state.block_entities[&key(chest)].slots[0].is_empty());
}

#[test]
fn hopper_furnace_insertion_uses_top_input_and_side_fuel_slots() {
    let mut state = state();
    let furnace = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "hopper", &[("facing", "east")]);
    place(&mut state, furnace, "furnace", &[]);
    let coal = item(&state, "coal", 2);
    set_slot(&mut state, HOPPER, 0, coal);
    step(&mut state);
    assert_eq!(state.block_entities[&key(furnace)].slots[1].count, 1);
    assert!(state.block_entities[&key(furnace)].slots[0].is_empty());
    let top = super::super::offset(furnace, (0, 1, 0)).unwrap();
    place(&mut state, top, "hopper", &[("facing", "down")]);
    let stone = item(&state, "stone", 2);
    set_slot(&mut state, top, 0, stone);
    step(&mut state);
    assert_eq!(state.block_entities[&key(furnace)].slots[0].count, 1);
    assert!(state.block_entities[&key(furnace)].slots[2].is_empty());
    assert_eq!(total(&state), 4);
}

#[test]
fn double_chest_signal_counts_empty_unopened_partner() {
    let mut state = state();
    let partner = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(
        &mut state,
        HOPPER,
        "chest",
        &[("facing", "north"), ("type", "left")],
    );
    place(
        &mut state,
        partner,
        "chest",
        &[("facing", "north"), ("type", "right")],
    );
    let full = item(&state, "stone", 64);
    state
        .block_entities
        .get_mut(&key(HOPPER))
        .unwrap()
        .slots
        .fill(full);
    state.block_entities.remove(&key(partner));
    assert_eq!(signal(&state, HOPPER), 8);
    assert_eq!(signal(&state, partner), 8);
}

#[test]
fn hopper_cooldown_pauses_across_chunk_eviction_instead_of_resetting() {
    let dir = std::env::temp_dir().join(format!("hopper-evict-{}", uuid::Uuid::new_v4()));
    let mut state = state_with_dir(Some(dir.clone()), true);
    place(&mut state, HOPPER, "hopper", &[]);
    state
        .redstone_containers
        .set_cooldown(key(HOPPER), 5, state.tick);
    checkpoint(&mut state);
    let capture = state.world.capture_dirty(&BTreeMap::new());
    let flushed = capture.write().unwrap();
    assert!(
        !flushed.is_empty(),
        "native checkpoint must write the hopper"
    );
    state.world.mark_flushed(&flushed);
    let evicted = state.world.evict_outside(&Default::default());
    assert!(evicted.contains(&ChunkPos { x: 0, z: 0 }));
    for _ in 0..3 {
        step(&mut state);
    }
    assert_eq!(
        state.redstone_containers.remaining(key(HOPPER), state.tick),
        5
    );
    state
        .world
        .ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let chunk = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    let decoded = decode_chunk(
        &state.block_entities,
        &state.redstone_containers,
        &state.registry,
        chunk,
        state.tick,
    )
    .unwrap();
    hydrate_chunk(
        &mut state.block_entities,
        &mut state.redstone_containers,
        &state.registry,
        chunk,
        state.tick,
        decoded,
    );
    step(&mut state);
    assert_eq!(
        state.redstone_containers.remaining(key(HOPPER), state.tick),
        4
    );
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dropper_receipt_keeps_full_eight_tick_hopper_cooldown() {
    let mut state = state();
    let hopper = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let chest = super::super::offset(hopper, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dropper", &[("facing", "east")]);
    place(&mut state, hopper, "hopper", &[("facing", "east")]);
    place(&mut state, chest, "chest", &[]);
    let stone = item(&state, "stone", 1);
    set_slot(&mut state, HOPPER, 0, stone);
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut Vec::new());
    run(&mut state, &mut Vec::new(), &mut Vec::new());
    assert_eq!(
        state.redstone_containers.remaining(key(hopper), state.tick),
        8
    );
    for _ in 0..7 {
        step(&mut state);
    }
    assert!(state.block_entities[&key(chest)].slots[0].is_empty());
    step(&mut state);
    assert_eq!(state.block_entities[&key(chest)].slots[0].count, 1);
    assert_eq!(total(&state), 1);
}

#[test]
fn dispenser_flint_wears_only_successful_ignition_and_publishes_target() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let below = super::super::offset(target, (0, -1, 0)).unwrap();
    place(&mut state, below, "stone", &[]);
    let flint = item(&state, "flint_and_steel", 1);
    set_slot(&mut state, HOPPER, 0, flint);
    let mut writes = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut Vec::new());
    assert_eq!(kind(&state, target), Some("fire"));
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].damage, Some(1));
    assert!(writes.iter().any(|(pos, _)| *pos == target));
    place(&mut state, target, "stone", &[]);
    let before = state.block_entities[&key(HOPPER)].slots[0].clone();
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0], before);
    assert!(spawned.is_empty());
}

#[test]
fn dispenser_tnt_priming_consumes_once_and_owns_spawn_publication() {
    let mut state = state();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let tnt = item(&state, "tnt", 2);
    set_slot(&mut state, HOPPER, 0, tnt);
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
    assert!(
        spawned.is_empty(),
        "TNT helper owns publication, not the caller"
    );
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert_eq!(output.changes.entities_spawned.len(), 1);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
}

#[test]
fn automation_hydration_does_not_preempt_native_furnace_progress() {
    let mut state = state();
    place(&mut state, HOPPER, "furnace", &[]);
    let bytes = encode(
        &state,
        HOPPER,
        "furnace",
        &state.block_entities[&key(HOPPER)].slots,
    );
    state.world.set_opaque_block_entity(HOPPER, Some(bytes));
    let chunk = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    let mut inventories = crate::redstone::custody::BlockEntities::default();
    let mut containers = ContainerState::default();
    let decoded = decode_chunk(
        &inventories,
        &containers,
        &state.registry,
        chunk,
        state.tick,
    )
    .unwrap();
    hydrate_chunk(
        &mut inventories,
        &mut containers,
        &state.registry,
        chunk,
        state.tick,
        decoded,
    );
    assert!(
        !inventories.contains_key(&key(HOPPER)),
        "furnace slots and timers must be hydrated together by the native furnace owner"
    );
}

#[test]
fn hopper_brewing_sides_keep_ingredient_fuel_and_bottle_custody_separate() {
    let mut state = state();
    let stand = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let top = super::super::offset(stand, (0, 1, 0)).unwrap();
    let bottom = super::super::offset(stand, (0, -1, 0)).unwrap();
    place(&mut state, HOPPER, "hopper", &[("facing", "east")]);
    place(&mut state, stand, "brewing_stand", &[]);
    place(&mut state, top, "hopper", &[("facing", "down")]);
    place(&mut state, bottom, "hopper", &[("enabled", "true")]);
    let powder = item(&state, "blaze_powder", 2);
    let wart = item(&state, "nether_wart", 2);
    set_slot(&mut state, HOPPER, 0, powder);
    set_slot(&mut state, top, 0, wart);
    step(&mut state);
    assert_eq!(state.block_entities[&key(stand)].slots[3].count, 1);
    assert_eq!(state.block_entities[&key(stand)].slots[4].count, 1);
    assert!(
        state.block_entities[&key(bottom)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    let bottle = item(&state, "potion", 1).with_custom_name("brewed");
    set_slot(&mut state, stand, 0, bottle.clone());
    step(&mut state);
    assert_eq!(state.block_entities[&key(bottom)].slots[0], bottle);
    assert!(state.block_entities[&key(stand)].slots[0].is_empty());
    assert_eq!(state.block_entities[&key(stand)].slots[3].count, 1);
    assert_eq!(state.block_entities[&key(stand)].slots[4].count, 1);
    assert_eq!(total(&state), 5);
}

#[test]
fn dispenser_flint_primes_tnt_without_duplicate_spawn_or_direct_air_publication() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    place(&mut state, target, "tnt", &[]);
    let flint = item(&state, "flint_and_steel", 1);
    set_slot(&mut state, HOPPER, 0, flint);
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].damage, Some(1));
    assert!(writes.is_empty());
    assert!(spawned.is_empty());
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert_eq!(output.changes.entities_spawned.len(), 1);
    assert!(
        output
            .changes
            .blocks
            .iter()
            .any(|change| change.position == target && change.state == state.registry.states.air)
    );
}

#[test]
fn s9_dispenser_bone_meal_grows_torchflower() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let below = super::super::offset(target, (0, -1, 0)).unwrap();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    place(&mut state, below, "farmland", &[("moisture", "7")]);
    place(&mut state, target, "torchflower_crop", &[("age", "0")]);
    let bone = item(&state, "bone_meal", 2);
    set_slot(&mut state, HOPPER, 0, bone);
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut Vec::new());
    let id = state.world.block_at(target.x, target.y, target.z).unwrap();
    assert_eq!(
        crate::farming::prop(&state.registry.blocks, id, "age"),
        Some("1")
    );
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
}

#[test]
fn dispenser_bone_meal_applies_complete_multicell_tree_plan_once() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let below = super::super::offset(target, (0, -1, 0)).unwrap();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    place(&mut state, below, "dirt", &[]);
    place(&mut state, target, "oak_sapling", &[("stage", "1")]);
    let expected = (0..128)
        .find_map(|tick| {
            state.tick = tick;
            crate::plants::bonemeal(&state, target, selection_seed(tick, target))
                .filter(|plan| plan.writes.len() > 1)
        })
        .expect("a supported sapling has a complete tree-growth plan");
    let bone = item(&state, "bone_meal", 2);
    set_slot(&mut state, HOPPER, 0, bone);
    let mut writes = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut Vec::new());
    for (pos, new) in expected.writes {
        assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(new));
    }
    assert!(writes.len() > 1);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
}

#[test]
fn hopper_pushes_into_and_pulls_from_all_seventeen_shulker_variants() {
    for path in mc_data::block_entity_kinds::BlockEntityKind::ShulkerBox.block_paths() {
        let mut state = state();
        let destination = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
        let above = super::super::offset(HOPPER, (0, 1, 0)).unwrap();
        place(
            &mut state,
            HOPPER,
            "hopper",
            &[("facing", "east"), ("enabled", "true")],
        );
        place(&mut state, destination, path, &[]);
        place(&mut state, above, path, &[]);
        let mut diamond = item(&state, "diamond", 4).with_custom_name("automation");
        let mut emerald = item(&state, "emerald", 3);
        set_slot(&mut state, HOPPER, 0, diamond.clone());
        set_slot(&mut state, above, 26, emerald.clone());
        step(&mut state);
        diamond.count = 1;
        emerald.count = 2;
        assert_eq!(
            state.block_entities[&key(destination)].slots[0],
            diamond,
            "{path}"
        );
        assert_eq!(
            state.block_entities[&key(above)].slots[26],
            emerald,
            "{path}"
        );
        assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 3);
        assert_eq!(state.block_entities[&key(HOPPER)].slots[1].count, 1);
        assert_eq!(total(&state), 7);
        crate::shulker::run(&mut state);
        let hydrated: BTreeMap<_, _> = crate::shulker::decode_chunk(
            &crate::redstone::custody::BlockEntities::default(),
            &state.registry,
            state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
        )
        .unwrap()
        .into_iter()
        .collect();
        assert_eq!(
            hydrated[&key(destination)],
            state.block_entities[&key(destination)].slots
        );
        assert_eq!(
            hydrated[&key(above)],
            state.block_entities[&key(above)].slots
        );
    }
}

#[test]
fn hopper_refuses_every_nested_shulker_without_moving_or_losing_it() {
    for path in mc_data::block_entity_kinds::BlockEntityKind::ShulkerBox.block_paths() {
        let mut state = state();
        let destination = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
        place(
            &mut state,
            HOPPER,
            "hopper",
            &[("facing", "east"), ("enabled", "true")],
        );
        place(&mut state, destination, "black_shulker_box", &[]);
        let box_item = item(&state, path, 1);
        set_slot(&mut state, HOPPER, 0, box_item.clone());
        step(&mut state);
        assert_eq!(
            state.block_entities[&key(HOPPER)].slots[0],
            box_item,
            "{path}"
        );
        assert!(
            state.block_entities[&key(destination)]
                .slots
                .iter()
                .all(ItemStack::is_empty)
        );
        assert_eq!(total(&state), 1);
    }
}

#[test]
fn dropper_inserts_into_shulker_and_refuses_nested_box_without_ejection() {
    let mut state = state();
    let destination = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dropper", &[("facing", "east")]);
    place(&mut state, destination, "shulker_box", &[]);
    let diamond = item(&state, "diamond", 2);
    set_slot(&mut state, HOPPER, 0, diamond);
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(state.block_entities[&key(destination)].slots[0].count, 1);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
    assert!(spawned.is_empty());
    let nested = item(&state, "blue_shulker_box", 1);
    set_slot(&mut state, HOPPER, 0, nested.clone());
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0], nested);
    assert_eq!(state.block_entities[&key(destination)].slots[0].count, 1);
    assert!(
        spawned.is_empty(),
        "blocked transfer must not eject a nested box"
    );
}

#[test]
fn built_base_cooling_hopper_wal_has_no_per_tick_timer_images() {
    use crate::journal::{JournalEntry, JournalWriter};
    let mut state = state();
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    // Isolate private timer revisions from unrelated terrain growth/decay.
    state
        .gamerules
        .insert("random_tick_speed".into(), "0".into());
    let chunks: rustc_hash::FxHashMap<_, _> = (-4..0)
        .flat_map(|x| (-4..0).map(move |z| (ChunkPos { x, z }, 0)))
        .collect();
    state.world.ensure_blocking(&chunks);
    for n in 0..64 {
        let x = -64 + (n % 8) * 8;
        let z = -64 + (n / 8) * 8;
        let a = BlockPos {
            x: x + 2,
            y: 201,
            z,
        };
        let b = BlockPos {
            x: x + 3,
            y: 201,
            z,
        };
        let chunk = ChunkPos {
            x: a.x.div_euclid(16),
            z: a.z.div_euclid(16),
        };
        state.tickets.insert(format!("stock-{n}"), (chunk, 0));
        place(&mut state, a, "hopper", &[("facing", "east")]);
        place(&mut state, b, "hopper", &[("facing", "west")]);
        let dirt = item(&state, "dirt", 64);
        set_slot(&mut state, a, 0, dirt.clone());
        set_slot(&mut state, a, 1, dirt);
    }
    let (out, _, _) = crate::tick::run_tick(&mut state, &[]);
    let _ = crate::core::journal_entries_for_tick(&mut state, &out);
    let revisions: Vec<_> = state
        .world
        .snapshot()
        .iter()
        .map(|c| (c.pos, c.revision))
        .collect();
    let dir = std::env::temp_dir().join(format!("hopper-timer-wal-{}", uuid::Uuid::new_v4()));
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    let mut opaque_per_tick = Vec::new();
    let mut bytes_per_tick = Vec::new();
    for _ in 0..7 {
        let (out, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut state, &out);
        opaque_per_tick.push(
            entries
                .iter()
                .filter(|e| matches!(e, JournalEntry::OpaqueBlockEntity { .. }))
                .count(),
        );
        writer.append(state.tick, state.tick, &entries).unwrap();
        writer.flush().unwrap();
        let path = dir
            .join("journal")
            .join(format!("batch-{:020}-{:020}.bin", state.tick, state.tick));
        bytes_per_tick.push(std::fs::metadata(path).unwrap().len());
    }
    eprintln!(
        "128 stocked cooling hoppers: opaque WAL entries/tick={opaque_per_tick:?}, complete WAL bytes/tick={bytes_per_tick:?}"
    );
    assert_eq!(total(&state), 8192);
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        opaque_per_tick,
        vec![0; 7],
        "cooldown elapsed time is derived, not journaled every tick"
    );
    for (pos, revision) in revisions {
        assert_eq!(
            state.world.chunk_revision(pos.x, pos.z),
            revision,
            "private hopper timers must not change wire revision"
        );
    }
}

#[test]
fn native_transfers_use_free_stock_then_stop_at_reserved_floor() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "barrel", &[]);
    place(&mut state, target, "hopper", &[]);
    let source = item(&state, "stone", 2);
    set_slot(&mut state, HOPPER, 0, source);
    crate::crafter::tests::hold_block(&mut state, HOPPER, "stone", 1);
    assert!(transfer_one(&mut state, HOPPER, target, (1, 0, 0), true));
    assert!(!transfer_one(&mut state, HOPPER, target, (1, 0, 0), true));
    let mut vehicle = vec![ItemStack::EMPTY; 5];
    assert!(!take_for_vehicle(&mut state, HOPPER, &mut vehicle));
    assert!(vehicle.iter().all(ItemStack::is_empty));
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].count, 1);
    assert_eq!(state.block_entities[&key(target)].slots[0].count, 1);
}

#[test]
fn double_chest_partner_extraction_preserves_joined_reservation_floor() {
    let mut state = state();
    let partner = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let target = super::super::offset(HOPPER, (0, -1, 0)).unwrap();
    place(
        &mut state,
        HOPPER,
        "chest",
        &[("facing", "north"), ("type", "left")],
    );
    place(
        &mut state,
        partner,
        "chest",
        &[("facing", "north"), ("type", "right")],
    );
    place(&mut state, target, "hopper", &[]);
    let source = item(&state, "stone", 2);
    set_slot(&mut state, partner, 0, source);
    crate::crafter::tests::hold_block(&mut state, HOPPER, "stone", 1);
    assert!(transfer_one(&mut state, partner, target, (0, -1, 0), true));
    assert!(!transfer_one(&mut state, partner, target, (0, -1, 0), true));
    assert_eq!(state.block_entities[&key(partner)].slots[0].count, 1);
}

#[test]
fn reserved_dispenser_bucket_has_no_fluid_or_spawn_effect() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let source = item(&state, "water_bucket", 1);
    set_slot(&mut state, HOPPER, 0, source.clone());
    crate::crafter::tests::hold_block(&mut state, HOPPER, "water_bucket", 1);
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert!(writes.is_empty());
    assert!(spawned.is_empty());
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.air)
    );
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0], source);
}

#[test]
fn reserved_dropper_flint_cannot_eject_but_dispenser_can_wear_without_breaking() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    let below = super::super::offset(target, (0, -1, 0)).unwrap();
    place(&mut state, HOPPER, "dropper", &[("facing", "east")]);
    place(&mut state, below, "stone", &[]);
    let source = item(&state, "flint_and_steel", 1);
    set_slot(&mut state, HOPPER, 0, source.clone());
    crate::crafter::tests::hold_block(&mut state, HOPPER, "flint_and_steel", 1);
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert!(spawned.is_empty());
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0], source);
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0].damage, Some(1));
    let max = mc_data::item_semantics_26_1_2::max_damage_for_stack(
        &state.registry.item_facts,
        &state.registry.items,
        &source,
    )
    .unwrap();
    state.block_entities.get_mut(&key(HOPPER)).unwrap().slots[0].damage = Some(max - 1);
    state
        .world
        .set_block(target.x, target.y, target.z, state.registry.states.air);
    let mut writes = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert!(writes.is_empty());
    assert!(spawned.is_empty());
    assert_eq!(
        state.block_entities[&key(HOPPER)].slots[0].damage,
        Some(max - 1)
    );
}

#[test]
fn dispenser_places_filled_shulker_and_keeps_blocked_box() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let contents = item(&state, "diamond", 17).with_custom_name("cargo");
    let box_item = item(&state, "blue_shulker_box", 1);
    let mut fields = stack_to_fields(&state.registry.items, &box_item).unwrap();
    fields.push((
        "components".into(),
        Tag::Compound(vec![(
            "minecraft:container".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("slot".into(), Tag::Int(26)),
                    (
                        "item".into(),
                        Tag::Compound(stack_to_fields(&state.registry.items, &contents).unwrap()),
                    ),
                ])],
            }),
        )]),
    ));
    let filled = stack_from_fields(&state.registry.items, &fields)
        .unwrap()
        .unwrap();
    set_slot(&mut state, HOPPER, 0, filled.clone());
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(kind(&state, target), Some("blue_shulker_box"));
    assert_eq!(state.block_entities[&key(target)].slots[26], contents);
    assert!(state.block_entities[&key(HOPPER)].slots[0].is_empty());
    assert!(spawned.is_empty());
    let block = super::super::at(&state, target).unwrap();
    assert_eq!(super::super::prop(block, "facing"), Some("east"));
    assert!(
        state
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .contains_key(&target)
    );
    set_slot(&mut state, HOPPER, 0, filled.clone());
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0], filled);
    assert!(spawned.is_empty());
}

#[test]
fn dispenser_equips_empty_mob_armor_slot_once_and_preserves_components() {
    let mut state = state();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let mob = crate::bees::tests::mob(&mut state, "minecraft:zombie", [4.5, 201.0, 3.5]);
    let slot = state.entities.slot_of(mob).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment = Default::default();
    state.entities.row_mut(slot).loot.can_pick_up_loot = true;
    let helmet = item(&state, "diamond_helmet", 1).with_custom_name("dispensed");
    set_slot(&mut state, HOPPER, 0, helmet.clone());
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(
        state.entities.row(slot).appearance.equipment[5],
        Some(helmet.clone())
    );
    assert_eq!(state.entities.row(slot).loot.drop_chances[5], 2.0);
    assert!(state.entities.row(slot).persistent);
    assert!(state.appearance_changed.contains(&mob));
    assert!(state.block_entities[&key(HOPPER)].slots[0].is_empty());
    assert!(spawned.is_empty());
    set_slot(&mut state, HOPPER, 0, helmet.clone());
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(
        state.entities.row(slot).appearance.equipment[5],
        Some(helmet)
    );
    assert_eq!(
        spawned.len(),
        1,
        "occupied armor slot ejects instead of replacing"
    );
}

#[test]
fn dispenser_waterlogs_and_drains_without_destroying_container_block() {
    let mut state = state();
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    place(&mut state, target, "oak_slab", &[("waterlogged", "false")]);
    let dry = state.world.block_at(target.x, target.y, target.z).unwrap();
    let bucket = item(&state, "water_bucket", 1);
    set_slot(&mut state, HOPPER, 0, bucket);
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(kind(&state, target), Some("oak_slab"));
    assert_eq!(
        super::super::prop(super::super::at(&state, target).unwrap(), "waterlogged"),
        Some("true")
    );
    assert_eq!(
        state.block_entities[&key(HOPPER)].slots[0],
        item(&state, "bucket", 1)
    );
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(dry)
    );
    assert_eq!(
        state.block_entities[&key(HOPPER)].slots[0],
        item(&state, "water_bucket", 1)
    );
    assert!(spawned.is_empty());
}

#[test]
fn dispenser_water_evaporates_in_nether_but_lava_roundtrips() {
    let mut state = state();
    state.dimension = crate::dimensions::Dimension::Nether;
    let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    let bucket = item(&state, "water_bucket", 1);
    set_slot(&mut state, HOPPER, 0, bucket);
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.air)
    );
    assert_eq!(
        state.block_entities[&key(HOPPER)].slots[0],
        item(&state, "bucket", 1)
    );
    let bucket = item(&state, "lava_bucket", 1);
    set_slot(&mut state, HOPPER, 0, bucket);
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.lava_levels[0])
    );
    trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.air)
    );
    assert_eq!(
        state.block_entities[&key(HOPPER)].slots[0],
        item(&state, "lava_bucket", 1)
    );
    assert!(spawned.is_empty());
}

#[test]
fn dispenser_bucket_can_empty_into_an_existing_source() {
    for fluid in ["water", "lava"] {
        let mut state = state();
        let target = super::super::offset(HOPPER, (1, 0, 0)).unwrap();
        place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
        place(&mut state, target, fluid, &[("level", "0")]);
        let full = item(&state, &format!("{fluid}_bucket"), 1);
        set_slot(&mut state, HOPPER, 0, full);
        let mut spawned = Vec::new();
        trigger(&mut state, HOPPER, &mut Vec::new(), &mut spawned);
        assert_eq!(kind(&state, target), Some(fluid));
        assert_eq!(
            state.block_entities[&key(HOPPER)].slots[0],
            item(&state, "bucket", 1)
        );
        assert!(spawned.is_empty());
    }
}

#[test]
fn dispenser_shulker_refuses_occupied_cell_without_debit_or_ejection() {
    let mut state = state();
    place(&mut state, HOPPER, "dispenser", &[("facing", "east")]);
    crate::bees::tests::mob(&mut state, "minecraft:zombie", [4.5, 201.0, 3.5]);
    let shulker = item(&state, "shulker_box", 1);
    set_slot(&mut state, HOPPER, 0, shulker.clone());
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    trigger(&mut state, HOPPER, &mut writes, &mut spawned);
    assert_eq!(state.block_entities[&key(HOPPER)].slots[0], shulker);
    assert!(writes.is_empty());
    assert!(spawned.is_empty());
}
