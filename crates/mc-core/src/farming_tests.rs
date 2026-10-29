//! Farming on a dirt platform in an otherwise empty world, full vanilla
//! data (crop loot tables, hoe durability).

use std::sync::Arc;

use mc_world::BlockStateId;

use crate::boundary::{BlockPos, ConnectionId, DigAction, EntityId, Hand, PlayerInput};
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;

/// Platform height; its top face is `Y + 1`.
pub(crate) const Y: i32 = 200;
/// Every cell of every section ticks about once per tick.
const FAST: &str = "4096";

struct EmptyWorld {
    air: u32,
}

impl mc_world::ChunkGenerator for EmptyWorld {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").expect("plains"),
        );
        chunk.status = "minecraft:full".into();
        chunk.dirty = true;
        chunk
    }
}

/// A joined survival player standing on a 21x21 dirt platform around the
/// origin, with a contained water source at (5, Y, 2).
pub(crate) fn setup() -> (TickState, EntityId) {
    let registry = Arc::new(crate::tick_tests::full_registry());
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(EmptyWorld {
        air: registry.states.air,
    });
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        BlockStateId(registry.states.air),
        [0, Y + 1, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "farmer".to_owned(),
            permission_level: 0,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let player = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("joined");
    let dirt = state.registry.states.dirt;
    for x in -10..=10 {
        for z in -10..=10 {
            state.world.set_block(x, Y, z, dirt);
        }
    }
    let stone = state.registry.states.stone;
    state.world.set_block(5, Y - 1, 2, stone);
    let water = state.registry.states.water_levels[0];
    state.world.set_block(5, Y, 2, water);
    state.world.fold_overlay();
    (state, player)
}

pub(crate) fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(format!("minecraft:{name}")).expect("block id");
    let props: Vec<_> = props
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    state
        .registry
        .blocks
        .by_name_and_props(&id, &props)
        .expect("block state")
        .0
}

pub(crate) fn put(state: &mut TickState, x: i32, y: i32, z: i32, id: u32) {
    state.world.set_block(x, y, z, id);
    state.world.fold_overlay();
}

fn plant(state: &mut TickState, x: i32, y: i32, z: i32, name: &str, props: &[(&str, &str)]) {
    let id = block(state, name, props);
    put(state, x, y, z, id);
}

fn at(state: &TickState, x: i32, y: i32, z: i32) -> u32 {
    state.world.block_at(x, y, z).expect("resident")
}

fn give(state: &mut TickState, player: EntityId, item: &str, count: i32) {
    let id = mc_data::Identifier::parse(format!("minecraft:{item}")).expect("item id");
    let item = state.registry.items.id_of(&id).expect("item");
    state
        .players
        .get_mut(&player)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(item, count);
}

fn held(state: &TickState, player: EntityId) -> mc_data::ItemStack {
    state.players[&player].inventory.slots[0].clone()
}

fn use_on(state: &mut TickState, x: i32, y: i32, z: i32, face: u8) {
    let _ = run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: BlockPos { x, y, z },
            face,
            cursor: [0.5, 1.0, 0.5],
            sequence: 1,
        }],
    );
}

fn near(state: &mut TickState, player: EntityId, x: i32, y: i32, z: i32) {
    let slot = state.entities.slot_of(player).expect("player slot");
    state.entities.set_pos(
        slot,
        [f64::from(x) + 0.5, f64::from(y) + 1.0, f64::from(z) + 0.5],
    );
}

fn dig(state: &mut TickState, x: i32, y: i32, z: i32) {
    crate::tick::mining_tests::ready_to_finish(state, ConnectionId(1), BlockPos { x, y, z });
    let _ = run_tick(
        state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: BlockPos { x, y, z },
            action: DigAction::Finish,
            sequence: 1,
        }],
    );
}

fn run(state: &mut TickState, ticks: usize) {
    for _ in 0..ticks {
        let _ = run_tick(state, &[]);
    }
}

/// Item entities on the ground as (item name, count), sorted.
fn dropped(state: &TickState) -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .map(|stack| {
            let name = state.registry.items.name_of(stack.item_id).expect("item");
            (name.path().to_owned(), stack.count)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn hoe_tills_dirt_and_grass_with_air_above_and_wears() {
    let (mut state, player) = setup();
    let farmland = block(&state, "farmland", &[("moisture", "0")]);
    give(&mut state, player, "wooden_hoe", 1);
    use_on(&mut state, 0, Y, 0, 1);
    assert_eq!(at(&state, 0, Y, 0), farmland, "dirt tilled");
    assert_eq!(held(&state, player).damage, Some(1), "one point of wear");

    let grass = state.registry.states.grass_block;
    put(&mut state, 1, Y, 0, grass);
    use_on(&mut state, 1, Y, 0, 1);
    assert_eq!(at(&state, 1, Y, 0), farmland, "grass tilled");
    assert_eq!(held(&state, player).damage, Some(2));

    let stone = state.registry.states.stone;
    let dirt = state.registry.states.dirt;
    put(&mut state, 2, Y, 0, dirt);
    put(&mut state, 2, Y + 1, 0, stone);
    use_on(&mut state, 2, Y, 0, 1);
    assert_eq!(
        at(&state, 2, Y, 0),
        state.registry.states.dirt,
        "covered dirt stays"
    );
    put(&mut state, 3, Y, 0, stone);
    use_on(&mut state, 3, Y, 0, 1);
    assert_eq!(at(&state, 3, Y, 0), stone, "stone is not tilled");
    assert_eq!(
        held(&state, player).damage,
        Some(2),
        "no wear without effect"
    );
}

#[test]
fn seeds_plant_on_farmland_not_on_dirt() {
    let (mut state, player) = setup();
    let air = state.registry.states.air;
    let farmland = block(&state, "farmland", &[("moisture", "0")]);
    put(&mut state, 1, Y, 0, farmland);
    put(&mut state, 2, Y, 0, farmland);
    put(&mut state, 3, Y, 0, farmland);
    put(&mut state, 4, Y, 0, farmland);

    give(&mut state, player, "wheat_seeds", 5);
    use_on(&mut state, 0, Y, 0, 1);
    assert_eq!(at(&state, 0, Y + 1, 0), air, "no seeds on dirt");
    assert_eq!(held(&state, player).count, 5, "nothing consumed");
    use_on(&mut state, 1, Y, 0, 1);
    assert_eq!(
        at(&state, 1, Y + 1, 0),
        block(&state, "wheat", &[("age", "0")])
    );
    assert_eq!(held(&state, player).count, 4);

    for (x, item, crop) in [
        (2, "carrot", "carrots"),
        (3, "potato", "potatoes"),
        (4, "beetroot_seeds", "beetroots"),
    ] {
        give(&mut state, player, item, 2);
        use_on(&mut state, x, Y, 0, 1);
        assert_eq!(
            at(&state, x, Y + 1, 0),
            block(&state, crop, &[("age", "0")]),
            "{item}"
        );
        assert_eq!(held(&state, player).count, 1);
    }

    give(&mut state, player, "wheat", 2);
    put(&mut state, 5, Y, 0, farmland);
    use_on(&mut state, 5, Y, 0, 1);
    assert_eq!(
        at(&state, 5, Y + 1, 0),
        air,
        "the wheat item places nothing"
    );
}

#[test]
fn random_ticks_grow_wheat_to_maturity_deterministically() {
    let ticks_to_mature = || {
        let (mut state, _) = setup();
        state
            .gamerules
            .insert("random_tick_speed".into(), FAST.into());
        let wet = block(&state, "farmland", &[("moisture", "7")]);
        for x in 1..=3 {
            for z in 1..=3 {
                put(&mut state, x, Y, z, wet);
            }
        }
        plant(&mut state, 2, Y + 1, 2, "wheat", &[("age", "0")]);
        let mature = block(&state, "wheat", &[("age", "7")]);
        for tick in 1..=600 {
            let _ = run_tick(&mut state, &[]);
            if at(&state, 2, Y + 1, 2) == mature {
                return tick;
            }
        }
        panic!("wheat did not mature in 600 ticks");
    };
    let first = ticks_to_mature();
    assert_eq!(first, ticks_to_mature(), "same world, same growth");
}

#[test]
fn default_random_tick_speed_is_three_cells_per_section() {
    let (mut state, _) = setup();
    let wheat = block(&state, "wheat", &[("age", "0")]);
    let wet = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 2, Y, 2, wet);
    put(&mut state, 2, Y + 1, 2, wheat);
    run(&mut state, 50);
    // 3 samples of 4096 cells per tick leave a lone crop almost untouched.
    assert!(at(&state, 2, Y + 1, 2) != block(&state, "wheat", &[("age", "7")]));
}

#[test]
fn zero_random_tick_speed_freezes_crop_growth_and_farmland_drying() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), "0".into());
    let wheat = block(&state, "wheat", &[("age", "0")]);
    let dry = block(&state, "farmland", &[("moisture", "0")]);
    put(&mut state, -8, Y, -8, dry);
    put(&mut state, -8, Y + 1, -8, wheat);
    run(&mut state, 100);
    assert_eq!(at(&state, -8, Y, -8), dry);
    assert_eq!(at(&state, -8, Y + 1, -8), wheat);
}

#[test]
fn bone_meal_advances_crops_and_is_spent() {
    let (mut state, player) = setup();
    let farmland = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 1, Y, 0, farmland);
    plant(&mut state, 1, Y + 1, 0, "wheat", &[("age", "0")]);
    give(&mut state, player, "bone_meal", 3);
    use_on(&mut state, 1, Y + 1, 0, 1);
    let aged = (2..=5)
        .find(|age| at(&state, 1, Y + 1, 0) == block(&state, "wheat", &[("age", &age.to_string())]))
        .expect("bone meal grows wheat by 2 to 5");
    assert!((2..=5).contains(&aged));
    assert_eq!(held(&state, player).count, 2, "one bone meal spent");

    plant(&mut state, 1, Y + 1, 0, "wheat", &[("age", "7")]);
    use_on(&mut state, 1, Y + 1, 0, 1);
    assert_eq!(held(&state, player).count, 2, "mature crops refuse it");
}

#[test]
fn mature_wheat_drops_wheat_and_seeds_immature_only_seeds() {
    let (mut state, _) = setup();
    let farmland = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 1, Y, 0, farmland);
    plant(&mut state, 1, Y + 1, 0, "wheat", &[("age", "7")]);
    dig(&mut state, 1, Y + 1, 0);
    let drops = dropped(&state);
    assert!(
        drops
            .iter()
            .any(|(name, count)| name == "wheat" && *count == 1),
        "mature wheat drops wheat: {drops:?}"
    );
    assert!(
        drops
            .iter()
            .all(|(name, _)| name == "wheat" || name == "wheat_seeds"),
        "{drops:?}"
    );

    let (mut state, _) = setup();
    put(&mut state, 1, Y, 0, farmland);
    plant(&mut state, 1, Y + 1, 0, "wheat", &[("age", "3")]);
    dig(&mut state, 1, Y + 1, 0);
    assert_eq!(dropped(&state), vec![("wheat_seeds".to_owned(), 1)]);
}

#[test]
fn farmland_near_water_soaks_and_far_dry_farmland_turns_to_dirt() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    let dry = block(&state, "farmland", &[("moisture", "0")]);
    put(&mut state, 2, Y, 2, dry);
    put(&mut state, -8, Y, -8, dry);
    put(&mut state, -8, Y, -6, dry);
    plant(&mut state, -8, Y + 1, -6, "wheat", &[("age", "0")]);
    run(&mut state, 20);
    assert_eq!(
        at(&state, 2, Y, 2),
        block(&state, "farmland", &[("moisture", "7")]),
        "water within four blocks"
    );
    assert_eq!(
        at(&state, -8, Y, -8),
        state.registry.states.dirt,
        "bare dry farmland dries out"
    );
    assert_eq!(at(&state, -8, Y, -6), dry, "a crop keeps its farmland");
}

#[test]
fn crops_off_farmland_pop_with_their_loot() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    plant(&mut state, 9, Y + 1, 9, "wheat", &[("age", "2")]);
    run(&mut state, 20);
    assert_eq!(at(&state, 9, Y + 1, 9), state.registry.states.air);
    assert_eq!(dropped(&state), vec![("wheat_seeds".to_owned(), 1)]);
}

#[test]
fn sapling_grows_an_oak_tree() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    plant(&mut state, -5, Y + 1, -5, "oak_sapling", &[("stage", "0")]);
    let log = block(&state, "oak_log", &[("axis", "y")]);
    for _ in 0..600 {
        let _ = run_tick(&mut state, &[]);
        if at(&state, -5, Y + 1, -5) == log {
            break;
        }
    }
    let trunk = (Y + 1..)
        .take_while(|y| at(&state, -5, *y, -5) == log)
        .count();
    assert!((4..=6).contains(&trunk), "trunk of {trunk} logs");
    let top = Y + trunk as i32;
    let leaves = |dx: i32, y: i32, dz: i32| {
        state
            .registry
            .blocks
            .by_id(BlockStateId(at(&state, -5 + dx, y, -5 + dz)))
            .is_some_and(|found| found.block.id.path() == "oak_leaves")
    };
    assert!(leaves(0, top + 1, 0), "crown above the trunk");
    assert!(leaves(2, top - 1, 0), "5x5 layer");
    assert!(!leaves(3, top - 1, 0), "no leaves beyond radius 2");
    assert_eq!(at(&state, -5, Y, -5), state.registry.states.dirt);
}

#[test]
fn grass_spreads_to_dirt_with_air_above() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    let grass = state.registry.states.grass_block;
    put(&mut state, -9, Y, -9, grass);
    let stone = state.registry.states.stone;
    put(&mut state, -8, Y + 1, -9, stone);
    run(&mut state, 30);
    assert_eq!(at(&state, -9, Y, -8), grass, "open dirt next to grass");
    assert_eq!(
        at(&state, -8, Y, -9),
        state.registry.states.dirt,
        "dirt with a block above stays dirt"
    );
}

#[test]
fn inert_section_becomes_tickable_only_after_overlay_commit() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    // Prime the negative eligibility cache on a section of inert dirt/air.
    run(&mut state, 1);
    let wheat = block(&state, "wheat", &[("age", "2")]);
    state.world.set_block(9, Y + 1, 9, wheat);
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    crate::farming::run(&mut state, &mut writes, &mut spawned);
    assert_eq!(
        at(&state, 9, Y + 1, 9),
        wheat,
        "same-tick overlay does not enable a previously inert section"
    );
    assert!(writes.is_empty());
    state.world.fold_overlay();
    run(&mut state, 20);
    assert_eq!(
        at(&state, 9, Y + 1, 9),
        state.registry.states.air,
        "new committed revision enables random ticks"
    );
    assert_eq!(dropped(&state), vec![("wheat_seeds".to_owned(), 1)]);
}

#[test]
fn open_sky_crops_grow_at_night_because_crops_gate_on_undarkened_light() {
    // Vanilla CropBlock.randomTick tests `getRawBrightness(pos, 0)`: no sky
    // darkening, so a night field under open sky keeps growing even though the
    // darkened spawn brightness there is 4.
    let (mut state, _) = setup();
    state.day_time = 18_000;
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    plant(&mut state, 0, Y, 0, "farmland", &[("moisture", "7")]);
    plant(&mut state, 0, Y + 1, 0, "wheat", &[("age", "0")]);
    let young = at(&state, 0, Y + 1, 0);
    assert_eq!(state.brightness(0, Y + 1, 0), 4);
    assert!(state.raw_brightness(0, Y + 1, 0) >= 9);
    run(&mut state, 40);
    assert_ne!(
        at(&state, 0, Y + 1, 0),
        young,
        "night does not stop an open-sky crop"
    );
}

#[path = "leaves_tests.rs"]
mod leaves_tests;
#[path = "plants_tests.rs"]
mod plants_tests;
#[path = "signs_tests.rs"]
mod signs_tests;

#[test]
fn bone_meal_reaches_column_tips_beyond_twenty_five_blocks() {
    let (mut state, player) = setup();
    // A 30-tall twisting vine column grown tip-up from the platform.
    let body = block(&state, "twisting_vines_plant", &[("age", "0")]);
    for y in 1..=29 {
        put(&mut state, 3, Y + y, 1, body);
    }
    let tip = block(&state, "twisting_vines", &[("age", "0")]);
    put(&mut state, 3, Y + 30, 1, tip);
    near(&mut state, player, 3, Y + 1, 1);
    give(&mut state, player, "bone_meal", 2);
    use_on(&mut state, 3, Y + 1, 1, 1);
    assert_eq!(
        at(&state, 3, Y + 31, 1),
        block(&state, "twisting_vines", &[("age", "1")]),
        "bone meal at the base grows a 30-block column tip"
    );
    assert_eq!(held(&state, player).count, 1);
}

#[test]
fn kelp_grows_into_falling_water_and_turns_it_into_a_source() {
    let (mut state, player) = setup();
    // A top-poured column: source over falling water over a kelp tip.
    let source = state.registry.states.water_levels[0];
    let falling = block(&state, "water", &[("level", "1")]);
    put(&mut state, -3, Y + 3, 1, source);
    put(&mut state, -3, Y + 2, 1, falling);
    let tip = block(&state, "kelp", &[("age", "0")]);
    put(&mut state, -3, Y + 1, 1, tip);
    near(&mut state, player, -3, Y + 1, 1);
    give(&mut state, player, "bone_meal", 2);
    use_on(&mut state, -3, Y + 1, 1, 1);
    assert_eq!(at(&state, -3, Y + 1, 1), block(&state, "kelp_plant", &[]));
    assert_eq!(
        at(&state, -3, Y + 2, 1),
        block(&state, "kelp", &[("age", "1")])
    );
    assert_eq!(held(&state, player).count, 1);
    // A single downward-flow cell over the kelp column with air above
    // still grows: the flow becomes a source and the tip extends into it.
    let air = state.registry.states.air;
    put(&mut state, -3, Y + 3, 1, air);
    give(&mut state, player, "bone_meal", 1);
    use_on(&mut state, -3, Y + 2, 1, 1);
    assert_eq!(at(&state, -3, Y + 1, 1), block(&state, "kelp_plant", &[]));
    assert_eq!(
        at(&state, -3, Y + 2, 1),
        block(&state, "kelp", &[("age", "1")])
    );
    // Sideways flow (horizontal current, no water column) still refuses.
    let kelp0 = block(&state, "kelp", &[("age", "0")]);
    let flow1 = block(&state, "water", &[("level", "1")]);
    let air0 = state.registry.states.air;
    put(&mut state, 6, Y + 1, 6, kelp0);
    put(&mut state, 6, Y + 2, 6, flow1);
    put(&mut state, 7, Y + 2, 6, air0);
    give(&mut state, player, "bone_meal", 1);
    use_on(&mut state, 6, Y + 1, 6, 1);
    assert_eq!(
        at(&state, 6, Y + 2, 6),
        block(&state, "water", &[("level", "1")])
    );
}

#[test]
fn wheat_seeds_refuse_planting_in_darkness_but_accept_light() {
    let (mut state, player) = setup();
    let wet = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 6, Y, 3, wet);
    near(&mut state, player, 6, Y + 1, 3);
    // A sealed room: floor under the plot, single wall ring at crop
    // height, and a double-thick lid (single thickness leaks to 8).
    let stone = state.registry.states.stone;
    put(&mut state, 6, Y - 1, 3, stone);
    for dx in -2i32..=2 {
        for dz in -2i32..=2 {
            if dx.abs() <= 1 && dz.abs() <= 1 && (dx != 0 || dz != 0) {
                put(&mut state, 6 + dx, Y + 1, 3 + dz, stone);
            }
            put(&mut state, 6 + dx, Y + 2, 3 + dz, stone);
            put(&mut state, 6 + dx, Y + 3, 3 + dz, stone);
        }
    }
    run(&mut state, 5);
    assert!(state.raw_brightness(6, Y + 1, 3) < 8, "roofed room is dark");
    give(&mut state, player, "wheat_seeds", 2);
    use_on(&mut state, 6, Y, 3, 1);
    let air = state.registry.states.air;
    assert_eq!(at(&state, 6, Y + 1, 3), air);
    assert_eq!(held(&state, player).count, 2, "dark planting refused");
    // Daylight restores planting.
    put(&mut state, 6, Y + 2, 3, air);
    put(&mut state, 6, Y + 3, 3, air);
    put(&mut state, 6, Y + 1, 2, air);
    run(&mut state, 5);
    assert!(state.raw_brightness(6, Y + 1, 3) >= 8);
    use_on(&mut state, 6, Y, 3, 1);
    assert_eq!(
        at(&state, 6, Y + 1, 3),
        block(&state, "wheat", &[("age", "0")])
    );
    assert_eq!(held(&state, player).count, 1);
}

#[test]
fn flowers_plant_on_soil_but_not_on_bare_stone() {
    let (mut state, player) = setup();
    let stone = state.registry.states.stone;
    near(&mut state, player, 7, Y + 1, 5);
    put(&mut state, 7, Y, 5, stone);
    give(&mut state, player, "poppy", 2);
    use_on(&mut state, 7, Y, 5, 1);
    assert_eq!(at(&state, 7, Y + 1, 5), state.registry.states.air);
    assert_eq!(held(&state, player).count, 2, "stone refuses flowers");
    let dirt = state.registry.states.dirt;
    put(&mut state, 8, Y, 5, dirt);
    use_on(&mut state, 8, Y, 5, 1);
    assert_eq!(at(&state, 8, Y + 1, 5), block(&state, "poppy", &[]));
    assert_eq!(held(&state, player).count, 1);
}

#[test]
fn bone_meal_duplicates_tall_flowers_without_removing_them() {
    let (mut state, player) = setup();
    let dirt = state.registry.states.dirt;
    put(&mut state, -5, Y, 0, dirt);
    let lower = block(&state, "sunflower", &[("half", "lower")]);
    let upper = block(&state, "sunflower", &[("half", "upper")]);
    put(&mut state, -5, Y + 1, 0, lower);
    put(&mut state, -5, Y + 2, 0, upper);
    near(&mut state, player, -5, Y + 1, 0);
    give(&mut state, player, "bone_meal", 1);
    use_on(&mut state, -5, Y + 1, 0, 1);
    assert_eq!(at(&state, -5, Y + 1, 0), lower, "plant retained");
    assert_eq!(at(&state, -5, Y + 2, 0), upper);
    assert!(
        dropped(&state)
            .iter()
            .any(|(name, count)| name == "sunflower" && *count == 1),
        "one sunflower drops: {:?}",
        dropped(&state)
    );
    assert!(held(&state, player).is_empty(), "one bone meal spent");
}

#[test]
fn sniffer_seeds_plant_their_crops_on_farmland() {
    let (mut state, player) = setup();
    let wet = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 0, Y, 6, wet);
    put(&mut state, 1, Y, 6, wet);
    near(&mut state, player, 0, Y + 1, 6);
    give(&mut state, player, "torchflower_seeds", 1);
    use_on(&mut state, 0, Y, 6, 1);
    assert_eq!(
        at(&state, 0, Y + 1, 6),
        block(&state, "torchflower_crop", &[("age", "0")])
    );
    assert!(held(&state, player).is_empty());
    give(&mut state, player, "pitcher_pod", 1);
    use_on(&mut state, 1, Y, 6, 1);
    assert_eq!(
        at(&state, 1, Y + 1, 6),
        block(&state, "pitcher_crop", &[("age", "0"), ("half", "lower")])
    );
    assert!(held(&state, player).is_empty());
}

#[test]
fn pitcher_pod_planting_preserves_moist_farmland() {
    let (mut state, player) = setup();
    let wet = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 1, Y, 6, wet);
    near(&mut state, player, 1, Y + 1, 6);
    give(&mut state, player, "pitcher_pod", 1);

    use_on(&mut state, 1, Y, 6, 1);
    assert_eq!(
        at(&state, 1, Y + 1, 6),
        block(&state, "pitcher_crop", &[("age", "0"), ("half", "lower")])
    );
    assert!(held(&state, player).is_empty(), "pod was planted");
    assert_eq!(at(&state, 1, Y, 6), wet, "planting must not cover farmland");

    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    run(&mut state, 10);
    assert_eq!(at(&state, 1, Y, 6), wet, "water keeps the soil moist");
}

#[test]
fn placed_vines_attach_to_the_clicked_wall_and_can_spread() {
    let (mut state, player) = setup();
    let wall = state.registry.states.stone;
    put(&mut state, 4, Y + 1, 7, wall);
    near(&mut state, player, 3, Y + 1, 7);
    give(&mut state, player, "vine", 2);
    // Click the west face of the stone so the vine lands against it.
    use_on(&mut state, 4, Y + 1, 7, 4);
    assert_eq!(
        at(&state, 3, Y + 1, 7),
        block(
            &state,
            "vine",
            &[
                ("east", "true"),
                ("north", "false"),
                ("south", "false"),
                ("up", "false"),
                ("west", "false"),
            ]
        ),
        "player-planted vine attaches east to the wall"
    );
    assert_eq!(held(&state, player).count, 1);
    // Click the ceiling above an empty cell: the vine hangs overhead.
    put(&mut state, 3, Y + 2, 6, wall);
    give(&mut state, player, "vine", 1);
    use_on(&mut state, 3, Y + 2, 6, 0);
    assert_eq!(
        at(&state, 3, Y + 1, 6),
        block(
            &state,
            "vine",
            &[
                ("east", "false"),
                ("north", "false"),
                ("south", "false"),
                ("up", "true"),
                ("west", "false"),
            ]
        ),
        "ceiling click hangs the vine overhead"
    );
}

#[test]
fn bone_meal_grows_azalea_and_fungi_but_rejects_wrong_soil() {
    let (mut state, player) = setup();
    let dirt = state.registry.states.dirt;
    put(&mut state, -6, Y, 2, dirt);
    plant(&mut state, -6, Y + 1, 2, "azalea", &[]);
    near(&mut state, player, -6, Y + 1, 2);
    // One 45% roll per meal: repeat until the tree lands.
    give(&mut state, player, "bone_meal", 20);
    let trunk = block(&state, "oak_log", &[("axis", "y")]);
    for _ in 0..20 {
        if at(&state, -6, Y + 2, 2) == trunk {
            break;
        }
        use_on(&mut state, -6, Y + 1, 2, 1);
        give(&mut state, player, "bone_meal", 20);
    }
    assert_eq!(at(&state, -6, Y + 2, 2), trunk, "azalea grows a trunk");
    // Crimson fungus on dirt refuses; on crimson nylium it grows huge.
    put(&mut state, -7, Y, 2, dirt);
    plant(&mut state, -7, Y + 1, 2, "crimson_fungus", &[]);
    give(&mut state, player, "bone_meal", 3);
    use_on(&mut state, -7, Y + 1, 2, 1);
    assert_eq!(held(&state, player).count, 3, "wrong soil refuses");
    // Far from the azalea canopy above (its leaves reach x=-8).
    let nylium = block(&state, "crimson_nylium", &[]);
    put(&mut state, 8, Y, 2, nylium);
    plant(&mut state, 8, Y + 1, 2, "crimson_fungus", &[]);
    near(&mut state, player, 8, Y + 1, 2);
    // One 40% roll per meal: repeat until the fungus lands.
    give(&mut state, player, "bone_meal", 20);
    let stem = block(&state, "crimson_stem", &[("axis", "y")]);
    for _ in 0..20 {
        if at(&state, 8, Y + 2, 2) == stem {
            break;
        }
        use_on(&mut state, 8, Y + 1, 2, 1);
        give(&mut state, player, "bone_meal", 20);
    }
    assert_eq!(
        at(&state, 8, Y + 2, 2),
        stem,
        "crimson fungus grows huge on its nylium"
    );
}

#[test]
fn bone_meal_spreads_moss_and_nylium_and_converts_netherrack() {
    let (mut state, player) = setup();
    let moss = block(&state, "moss_block", &[]);
    let stone = state.registry.states.stone;
    put(&mut state, 5, Y, 8, moss);
    near(&mut state, player, 5, Y + 1, 8);
    for dx in -2..=2 {
        for dz in -2..=2 {
            if dx == 0 && dz == 0 {
                continue;
            }
            put(&mut state, 5 + dx, Y, 8 + dz, stone);
        }
    }
    give(&mut state, player, "bone_meal", 8);
    for sequence in 0..8 {
        let _ = sequence;
        use_on(&mut state, 5, Y, 8, 1);
        let converted = (-2..=2)
            .flat_map(|dx| (-2..=2).map(move |dz| (dx, dz)))
            .filter(|(dx, dz)| (*dx != 0 || *dz != 0) && at(&state, 5 + dx, Y, 8 + dz) == moss)
            .count();
        if converted > 0 {
            break;
        }
    }
    let converted = (-2..=2)
        .flat_map(|dx| (-2..=2).map(move |dz| (dx, dz)))
        .filter(|(dx, dz)| (*dx != 0 || *dz != 0) && at(&state, 5 + dx, Y, 8 + dz) == moss)
        .count();
    assert!(converted > 0, "moss spreads to neighboring stone");
    // Netherrack beside nylium converts.
    let nylium = block(&state, "warped_nylium", &[]);
    let netherrack = state
        .registry
        .blocks
        .by_name_and_props(
            &mc_data::Identifier::parse("minecraft:netherrack").unwrap(),
            &[],
        )
        .unwrap()
        .0;
    put(&mut state, -5, Y, 8, netherrack);
    near(&mut state, player, -5, Y + 1, 8);
    put(&mut state, -4, Y, 8, nylium);
    give(&mut state, player, "bone_meal", 2);
    use_on(&mut state, -5, Y, 8, 1);
    assert_eq!(
        at(&state, -5, Y, 8),
        nylium,
        "netherrack converts to nylium"
    );
}

#[test]
fn bone_meal_grows_waterlogged_sea_pickle_colonies() {
    let (mut state, player) = setup();
    // A shallow tank: live coral floor, water above, glass walls.
    let coral = block(&state, "tube_coral_block", &[]);
    for dx in -1..=1 {
        for dz in -1..=1 {
            put(&mut state, dx, Y, -6 + dz, coral);
        }
    }
    let water = state.registry.states.water_levels[0];
    for dx in -1..=1 {
        for dz in -1..=1 {
            put(&mut state, dx, Y + 1, -6 + dz, water);
            put(&mut state, dx, Y + 2, -6 + dz, water);
        }
    }
    let pickle = block(
        &state,
        "sea_pickle",
        &[("pickles", "1"), ("waterlogged", "true")],
    );
    put(&mut state, 0, Y + 1, -6, pickle);
    near(&mut state, player, 0, Y + 1, -6);
    give(&mut state, player, "bone_meal", 8);
    for _ in 0..8 {
        use_on(&mut state, 0, Y + 1, -6, 1);
        let grown = at(&state, 0, Y + 1, -6);
        let spread = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
            .filter(|(dx, dz)| *dx != 0 || *dz != 0)
            .any(|(dx, dz)| {
                state
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(at(&state, dx, Y + 1, -6 + dz)))
                    .is_some_and(|block| block.block.id.path() == "sea_pickle")
            });
        if grown != pickle || spread {
            break;
        }
    }
    let grown = at(&state, 0, Y + 1, -6);
    let spread = (-1..=1)
        .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
        .filter(|(dx, dz)| *dx != 0 || *dz != 0)
        .any(|(dx, dz)| {
            state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(at(&state, dx, Y + 1, -6 + dz)))
                .is_some_and(|block| block.block.id.path() == "sea_pickle")
        });
    assert!(grown != pickle || spread, "colony grows or spreads");
}

#[test]
fn dripstone_with_lava_source_fills_the_cauldron_below() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    // Lava source over a dripstone block, down tip, empty cauldron below.
    let lava = state.registry.states.lava_levels[0];
    let dripstone_block = block(&state, "dripstone_block", &[]);
    let tip = block(
        &state,
        "pointed_dripstone",
        &[
            ("thickness", "tip"),
            ("vertical_direction", "down"),
            ("waterlogged", "false"),
        ],
    );
    let cauldron = block(&state, "cauldron", &[]);
    put(&mut state, 0, Y + 6, -8, lava);
    put(&mut state, 0, Y + 5, -8, dripstone_block);
    put(&mut state, 0, Y + 4, -8, tip);
    put(&mut state, 0, Y + 1, -8, cauldron);
    let lava_cauldron = block(&state, "lava_cauldron", &[]);
    for _ in 0..400 {
        run(&mut state, 1);
        if at(&state, 0, Y + 1, -8) == lava_cauldron {
            return;
        }
    }
    panic!("dripstone never filled the lava cauldron");
}
#[test]
fn dripstone_mid_column_tip_does_not_grow_but_top_tip_fills() {
    // Pinned PointedDripstoneBlock.isStalactiteStartPos: only the column
    // top (no pointed dripstone above) ticks; it fills through findTip, and
    // the tip below it never runs its own transfer or growth. Lava fill is
    // the control path.
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    let lava = state.registry.states.lava_levels[0];
    let dripstone_block = block(&state, "dripstone_block", &[]);
    let top = block(
        &state,
        "pointed_dripstone",
        &[
            ("thickness", "frustum"),
            ("vertical_direction", "down"),
            ("waterlogged", "false"),
        ],
    );
    let mid = block(
        &state,
        "pointed_dripstone",
        &[
            ("thickness", "tip"),
            ("vertical_direction", "down"),
            ("waterlogged", "false"),
        ],
    );
    let cauldron = block(&state, "cauldron", &[]);
    put(&mut state, 0, Y + 7, -10, lava);
    put(&mut state, 0, Y + 6, -10, dripstone_block);
    put(&mut state, 0, Y + 5, -10, top);
    put(&mut state, 0, Y + 4, -10, mid);
    put(&mut state, 0, Y + 1, -10, cauldron);
    let lava_cauldron = block(&state, "lava_cauldron", &[]);
    for _ in 0..400 {
        run(&mut state, 1);
        if at(&state, 0, Y + 1, -10) == lava_cauldron {
            // Mid-column tip must still be a tip (never grew into middle).
            assert_eq!(at(&state, 0, Y + 4, -10), mid);
            return;
        }
    }
    panic!("dripstone never filled the lava cauldron");
}

#[test]
fn budding_amethyst_sprouts_buds_that_advance_to_clusters() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    let budding = block(&state, "budding_amethyst", &[]);
    put(&mut state, 0, Y + 1, -9, budding);
    for _ in 0..600 {
        run(&mut state, 1);
        let grown = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dz)| {
            let (dx, dz) = (*dx, *dz);
            state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(at(&state, dx, Y + 1, -9 + dz)))
                .is_some_and(|block| {
                    matches!(
                        block.block.id.path(),
                        "small_amethyst_bud"
                            | "medium_amethyst_bud"
                            | "large_amethyst_bud"
                            | "amethyst_cluster"
                    )
                })
        }) || [0, 1, 2].iter().any(|dy| {
            state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(at(&state, 0, Y + dy, -9)))
                .is_some_and(|block| {
                    matches!(
                        block.block.id.path(),
                        "small_amethyst_bud"
                            | "medium_amethyst_bud"
                            | "large_amethyst_bud"
                            | "amethyst_cluster"
                    )
                })
        });
        if grown {
            return;
        }
    }
    panic!("budding amethyst never sprouted");
}

#[test]
fn turtle_eggs_crack_at_night_into_baby_turtles() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    state.day_time = 15_000;
    let sand = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:sand").unwrap())
        .unwrap()
        .default
        .0;
    // Sand is a falling block: bed it on stone like a real beach, or it
    // drops away and the egg (correctly) never hatches on the bare air.
    let stone = state.registry.states.stone;
    put(&mut state, 2, Y - 1, -2, stone);
    put(&mut state, 2, Y, -2, sand);
    let eggs = block(&state, "turtle_egg", &[("eggs", "2"), ("hatch", "0")]);
    put(&mut state, 2, Y + 1, -2, eggs);
    for _ in 0..3000 {
        run(&mut state, 1);
        let slots: Vec<_> = state.entities.slots().collect();
        let babies = slots
            .iter()
            .filter(|slot| {
                let row = state.entities.row(**slot);
                let body = state.entities.body(**slot);
                row.entity_life.child_ticks > 0
                    && matches!(body.kind, crate::entities::EntityKind::Mob { .. })
            })
            .count();
        if babies >= 2 {
            return;
        }
    }
    panic!("turtle eggs never hatched");
}

#[test]
fn sniffer_eggs_hatch_into_baby_sniffers() {
    // Vanilla SnifferEggBlock: scheduled thirds of 24000 ticks (12000 on
    // moss); each third cracks hatch+1, the third hatches a snifferlet.
    let (mut state, _) = setup();
    let egg = block(&state, "sniffer_egg", &[("hatch", "0")]);
    put(&mut state, 3, Y + 1, -2, egg);
    crate::eggs::changed(
        &mut state,
        crate::boundary::BlockPos {
            x: 3,
            y: Y + 1,
            z: -2,
        },
    );
    for _ in 0..30_000 {
        run(&mut state, 1);
        let babies = state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).entity_life.child_ticks > 0)
            .count();
        if babies >= 1 {
            return;
        }
    }
    panic!("sniffer egg never hatched");
}

#[test]
fn frogspawn_hatches_tadpoles_in_water() {
    // Vanilla FrogspawnBlock: one scheduled tick 3600..12000 hatches 2-5
    // tadpoles; no random-tick clock.
    let (mut state, _) = setup();
    let water = state.registry.states.water_levels[0];
    put(&mut state, 4, Y, -2, water);
    let spawn = block(&state, "frogspawn", &[]);
    put(&mut state, 4, Y + 1, -2, spawn);
    crate::eggs::changed(
        &mut state,
        crate::boundary::BlockPos {
            x: 4,
            y: Y + 1,
            z: -2,
        },
    );
    for _ in 0..15_000 {
        run(&mut state, 1);
        let kinds: Vec<_> = state
            .entities
            .slots()
            .map(|slot| state.entities.body(slot).kind)
            .collect();
        let tadpoles = kinds
            .iter()
            .filter(|kind| match kind {
                crate::entities::EntityKind::Mob { type_id } => state
                    .registry
                    .blueprint(*type_id)
                    .is_some_and(|blueprint| blueprint.name == "minecraft:tadpole"),
                _ => false,
            })
            .count();
        if tadpoles >= 2 {
            return;
        }
    }
    panic!("frogspawn never hatched");
}

#[test]
fn bone_meal_cadence_gates_azalea_fungus_and_mangrove() {
    // Azalea: one 45% roll per meal; success grows, failure consumes.
    let (mut state, player) = setup();
    let dirt = state.registry.states.dirt;
    put(&mut state, -6, Y, 2, dirt);
    plant(&mut state, -6, Y + 1, 2, "azalea", &[]);
    near(&mut state, player, -6, Y + 1, 2);
    give(&mut state, player, "bone_meal", 20);
    for _ in 0..20 {
        if at(&state, -6, Y + 2, 2) == block(&state, "oak_log", &[("axis", "y")]) {
            break;
        }
        use_on(&mut state, -6, Y + 1, 2, 1);
    }
    assert_eq!(
        at(&state, -6, Y + 2, 2),
        block(&state, "oak_log", &[("axis", "y")]),
        "repeated meals grow the azalea (45% each)"
    );
    // A single meal must be able to fail while still consuming the item.
    let (mut state, player) = setup();
    let dirt0 = state.registry.states.dirt;
    put(&mut state, -6, Y, 2, dirt0);
    plant(&mut state, -6, Y + 1, 2, "azalea", &[]);
    near(&mut state, player, -6, Y + 1, 2);
    give(&mut state, player, "bone_meal", 1);
    use_on(&mut state, -6, Y + 1, 2, 1);
    assert_eq!(
        held(&state, player).count,
        0,
        "meal is spent even on a miss"
    );
    // Huge fungus: 40% per meal on matching nylium.
    let (mut state, player) = setup();
    let nylium = block(&state, "crimson_nylium", &[]);
    put(&mut state, 8, Y, 2, nylium);
    plant(&mut state, 8, Y + 1, 2, "crimson_fungus", &[]);
    near(&mut state, player, 8, Y + 1, 2);
    give(&mut state, player, "bone_meal", 20);
    for _ in 0..20 {
        if at(&state, 8, Y + 2, 2) == block(&state, "crimson_stem", &[("axis", "y")]) {
            break;
        }
        use_on(&mut state, 8, Y + 1, 2, 1);
    }
    assert_eq!(
        at(&state, 8, Y + 2, 2),
        block(&state, "crimson_stem", &[("axis", "y")]),
        "repeated meals grow the fungus (40% each)"
    );
    // Hanging propagule ages instead of growing a tree.
    let (mut state, player) = setup();
    plant(
        &mut state,
        2,
        Y + 1,
        4,
        "mangrove_propagule",
        &[
            ("hanging", "true"),
            ("age", "0"),
            ("stage", "0"),
            ("waterlogged", "false"),
        ],
    );
    near(&mut state, player, 2, Y + 1, 4);
    give(&mut state, player, "bone_meal", 2);
    use_on(&mut state, 2, Y + 1, 4, 1);
    assert_eq!(
        at(&state, 2, Y + 1, 4),
        block(
            &state,
            "mangrove_propagule",
            &[
                ("hanging", "true"),
                ("age", "1"),
                ("stage", "0"),
                ("waterlogged", "false")
            ]
        ),
        "hanging propagule ages one stage"
    );
}

#[test]
fn ice_random_tick_melts_in_block_light() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    let ice = block(&state, "ice", &[]);
    let glowstone = block(&state, "glowstone", &[]);
    put(&mut state, 0, Y + 1, 0, ice);
    put(&mut state, -9, Y + 1, 0, ice);
    put(&mut state, 1, Y + 1, 0, glowstone);
    run(&mut state, 30);
    assert_eq!(at(&state, 0, Y + 1, 0), state.registry.states.water);
    assert_eq!(
        at(&state, -9, Y + 1, 0),
        ice,
        "sky light alone must not melt ice"
    );
}

#[test]
fn snow_random_tick_melts_in_block_light() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    let snow = block(&state, "snow", &[("layers", "1")]);
    let glowstone = block(&state, "glowstone", &[]);
    put(&mut state, 0, Y + 1, 0, snow);
    put(&mut state, -9, Y + 1, 0, snow);
    put(&mut state, 1, Y + 1, 0, glowstone);
    run(&mut state, 30);
    assert_eq!(at(&state, 0, Y + 1, 0), state.registry.states.air);
    assert_eq!(
        at(&state, -9, Y + 1, 0),
        snow,
        "sky light alone must not melt snow"
    );
}
