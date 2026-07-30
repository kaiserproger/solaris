use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use mc_data::Identifier;
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::tags::TagsData;
use mc_protocol::packets::play::{ContainerInput, ItemStack, ServerboundContainerClick};
use mc_world::{BlockStateId, Chunk, ChunkPos, FurnaceBlockEntity};
use tokio::sync::mpsc;

use crate::login::LoggedInProfile;
use crate::server::ServerConfig;

use super::{ActiveContainer, BlockDelta, FurnaceKind, FurnaceTickPlan, FurnaceWindow};
use super::{OutboundCommand, PlayerInventory, PlayerPose, SessionRegistry};
use super::{apply_furnace_swap_click, apply_furnace_throw_click};
use super::{commit_resident_furnace_tick_wave, decode_container_set_content_packets};
use super::{furnace_block, furnace_experience_award, furnace_fuel_ticks};
use super::{furnace_output_was_taken, furnace_slot_stacks, furnace_slot_to_stack};
use super::{furnace_tick_block_state, handle_furnace_container_click};
use super::{interaction_state_for_items, play_loop_slow_client_test_config};
use super::{simple_block, simulation_channel, stack_to_furnace_slot, tick_furnace};

#[test]
fn furnace_window_swap_and_throw_mutate_menu_slots() {
    let coal = Identifier::parse("minecraft:coal").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: coal,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let coal_id = items
        .id_of(&Identifier::parse("minecraft:coal").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(coal_id, 4);
    let mut furnace = FurnaceBlockEntity::default();

    assert!(apply_furnace_swap_click(
        &mut state,
        &mut furnace,
        FurnaceKind::Furnace,
        1,
        0,
    ));
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(coal_id, 4)
    );
    assert!(state.inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty());

    let dropped = apply_furnace_throw_click(&mut state, &mut furnace, 1, 0).unwrap();
    assert_eq!(dropped, ItemStack::new(coal_id, 1));
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(coal_id, 3)
    );
}

#[test]
fn furnace_uses_vanilla_common_fuel_times_and_returns_lava_bucket() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let raw_food = Identifier::parse("minecraft:raw_food").unwrap();
    let cooked_food = Identifier::parse("minecraft:cooked_food").unwrap();
    let item_names = [
        raw_food.clone(),
        cooked_food.clone(),
        Identifier::parse("minecraft:stick").unwrap(),
        Identifier::parse("minecraft:birch_planks").unwrap(),
        Identifier::parse("minecraft:wooden_pickaxe").unwrap(),
        Identifier::parse("minecraft:coal").unwrap(),
        Identifier::parse("minecraft:lava_bucket").unwrap(),
        Identifier::parse("minecraft:bucket").unwrap(),
        Identifier::parse("minecraft:oak_stairs").unwrap(),
        Identifier::parse("minecraft:oak_slab").unwrap(),
        Identifier::parse("minecraft:chest").unwrap(),
        Identifier::parse("minecraft:oak_door").unwrap(),
        Identifier::parse("minecraft:oak_boat").unwrap(),
        Identifier::parse("minecraft:white_wool").unwrap(),
        Identifier::parse("minecraft:white_carpet").unwrap(),
        Identifier::parse("minecraft:dried_kelp_block").unwrap(),
        Identifier::parse("minecraft:bamboo").unwrap(),
        Identifier::parse("minecraft:warped_planks").unwrap(),
        Identifier::parse("minecraft:warped_stairs").unwrap(),
    ];
    let reports = item_names
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, id)| ItemReport {
            id,
            protocol_id: u32::try_from(index + 1).unwrap(),
        })
        .collect::<Vec<_>>();
    let items = ItemRegistry::from_report(&reports);
    let raw_food_id = items.id_of(&raw_food).unwrap();
    let recipe = Recipe {
        id: Identifier::parse("minecraft:test_food").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(raw_food)],
            },
            cooking_time: 200,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_food,
            count: 1,
        },
    };
    let tags = mc_data::tags::solaris_required_item_tags(&items);

    for (fuel_name, expected_ticks) in [
        ("minecraft:stick", 100),
        ("minecraft:birch_planks", 300),
        ("minecraft:wooden_pickaxe", 200),
        ("minecraft:coal", 1600),
        ("minecraft:lava_bucket", 20_000),
        ("minecraft:oak_stairs", 300),
        ("minecraft:oak_slab", 150),
        ("minecraft:chest", 300),
        ("minecraft:oak_door", 200),
        ("minecraft:oak_boat", 1_200),
        ("minecraft:white_wool", 100),
        ("minecraft:white_carpet", 67),
        ("minecraft:dried_kelp_block", 4_001),
        ("minecraft:bamboo", 50),
    ] {
        let fuel_id = items.id_of(&Identifier::parse(fuel_name).unwrap()).unwrap();
        let mut furnace = FurnaceBlockEntity {
            burn_total: 0,
            ..FurnaceBlockEntity::default()
        };
        furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(raw_food_id, 1));
        furnace.slots[1] = stack_to_furnace_slot(&ItemStack::new(fuel_id, 1));

        let _ = tick_furnace(
            std::slice::from_ref(&recipe),
            &items,
            &tags,
            &mut furnace,
            FurnaceKind::Furnace,
        );

        assert_eq!(
            furnace.burn_total, expected_ticks,
            "wrong burn duration for {fuel_name}"
        );
        assert_eq!(furnace.burn_remaining, expected_ticks);
        if fuel_name == "minecraft:lava_bucket" {
            assert_eq!(
                furnace_slot_to_stack(&furnace.slots[1]),
                ItemStack::new(
                    items
                        .id_of(&Identifier::parse("minecraft:bucket").unwrap())
                        .unwrap(),
                    1,
                )
            );
        } else {
            assert!(furnace.slots[1].is_empty());
        }
    }

    let coal_id = items
        .id_of(&Identifier::parse("minecraft:coal").unwrap())
        .unwrap();
    assert_eq!(
        furnace_fuel_ticks(&tags, FurnaceKind::Smoker, coal_id),
        Some(800)
    );
    assert_eq!(
        furnace_fuel_ticks(&tags, FurnaceKind::BlastFurnace, coal_id),
        Some(800)
    );

    let warped_planks = items
        .id_of(&Identifier::parse("minecraft:warped_planks").unwrap())
        .unwrap();
    let mut furnace = FurnaceBlockEntity {
        burn_total: 0,
        ..FurnaceBlockEntity::default()
    };
    furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(raw_food_id, 1));
    furnace.slots[1] = stack_to_furnace_slot(&ItemStack::new(warped_planks, 1));
    let _ = tick_furnace(
        std::slice::from_ref(&recipe),
        &items,
        &tags,
        &mut furnace,
        FurnaceKind::Furnace,
    );
    assert_eq!(furnace.burn_total, 0);
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(warped_planks, 1)
    );

    let warped_stairs = items
        .id_of(&Identifier::parse("minecraft:warped_stairs").unwrap())
        .unwrap();
    furnace.slots[1] = stack_to_furnace_slot(&ItemStack::new(warped_stairs, 1));
    let _ = tick_furnace(
        std::slice::from_ref(&recipe),
        &items,
        &tags,
        &mut furnace,
        FurnaceKind::Furnace,
    );
    assert_eq!(furnace.burn_total, 0);
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(warped_stairs, 1)
    );
}

#[test]
fn furnace_cools_partial_progress_when_the_fuel_slot_is_empty() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let raw_food = Identifier::parse("minecraft:raw_food").unwrap();
    let cooked_food = Identifier::parse("minecraft:cooked_food").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: raw_food.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: cooked_food.clone(),
            protocol_id: 2,
        },
    ]);
    let recipe = Recipe {
        id: Identifier::parse("minecraft:test_food").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(raw_food.clone())],
            },
            cooking_time: 200,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_food,
            count: 1,
        },
    };
    let mut furnace = FurnaceBlockEntity {
        burn_remaining: 0,
        burn_total: 0,
        cook_progress: 50,
        cook_total: 200,
        ..FurnaceBlockEntity::default()
    };
    furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(items.id_of(&raw_food).unwrap(), 1));

    let _ = tick_furnace(
        std::slice::from_ref(&recipe),
        &items,
        &mc_data::tags::TagsData::default(),
        &mut furnace,
        FurnaceKind::Furnace,
    );

    assert_eq!(furnace.cook_progress, 48);
}

#[test]
fn completed_furnace_cook_records_the_recipe_for_experience() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let raw_iron = Identifier::parse("minecraft:raw_iron").unwrap();
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: raw_iron.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: iron_ingot.clone(),
            protocol_id: 2,
        },
    ]);
    let recipe = Recipe {
        id: Identifier::parse("minecraft:iron_ingot_from_smelting_raw_iron").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(raw_iron)],
            },
            cooking_time: 200,
            experience_milli: 700,
        }),
        result: RecipeResult {
            item: iron_ingot,
            count: 1,
        },
    };
    let mut furnace = FurnaceBlockEntity {
        burn_remaining: 2,
        cook_progress: 199,
        ..FurnaceBlockEntity::default()
    };
    furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(1, 1));

    let _ = tick_furnace(
        std::slice::from_ref(&recipe),
        &items,
        &TagsData::default(),
        &mut furnace,
        FurnaceKind::Furnace,
    );

    assert_eq!(
        furnace
            .recipes_used
            .get("minecraft:iron_ingot_from_smelting_raw_iron"),
        Some(&1)
    );
}

#[test]
fn taking_furnace_output_awards_only_recorded_furnace_recipe_experience() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let ingredient = Ingredient {
        alternatives: vec![IngredientAlternative::Item(
            Identifier::parse("minecraft:raw_iron").unwrap(),
        )],
    };
    let result = RecipeResult {
        item: Identifier::parse("minecraft:iron_ingot").unwrap(),
        count: 1,
    };
    let furnace_recipe = Recipe {
        id: Identifier::parse("minecraft:test_furnace").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: ingredient.clone(),
            cooking_time: 200,
            experience_milli: 1_000,
        }),
        result: result.clone(),
    };
    let campfire_recipe = Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient,
            cooking_time: 600,
            experience_milli: 1_000,
        }),
        result,
    };
    let recipes_used = BTreeMap::from([
        ("minecraft:test_furnace".to_string(), 2),
        ("minecraft:test_campfire".to_string(), 5),
    ]);
    let mut before = FurnaceBlockEntity::default();
    before.slots[2] = stack_to_furnace_slot(&ItemStack::new(2, 3));
    let mut after = before.clone();
    after.slots[2].count = 2;

    assert!(furnace_output_was_taken(&before, &after));
    assert_eq!(
        furnace_experience_award(&[furnace_recipe, campfire_recipe], &recipes_used, 0),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_furnace_tick_does_not_wait_for_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = Arc::clone(&blocks);
    state.world = Arc::clone(&world);
    state.world_read = world_read;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_furnace_block_entity(position, FurnaceBlockEntity::default())
            .unwrap();
    }

    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(69),
        name: "IdleFurnace".to_string(),
    };
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = state.sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    state.sessions.mark_loaded(session_id, (0, 0));

    let config = ServerConfig {
        blocks: Arc::clone(&state.blocks),
        world: Some(Arc::clone(&world)),
        tags: Arc::clone(&state.tags),
        recipes: Arc::new(state.recipes.clone()),
        items: Arc::clone(&state.items),
        block_facts: Arc::clone(&state.block_facts),
        ..play_loop_slow_client_test_config()
    };
    let world_guard = world.lock().await;
    let (_simulation, owner) = simulation_channel();
    let mut tick = Box::pin(owner.run_furnace_ticks(
        &config,
        &state.sessions,
        Some(&state.world_read),
        Some(&world_mutation),
    ));
    std::future::poll_fn(|cx| match std::future::Future::poll(tick.as_mut(), cx) {
        Poll::Ready(updated) => {
            assert_eq!(updated, 0);
            Poll::Ready(())
        }
        Poll::Pending => panic!("idle furnace tick waited for the world writer"),
    })
    .await;
    drop(world_guard);
}

#[tokio::test]
async fn active_furnace_tick_updates_resident_state_without_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            furnace_block(1, 2),
        ])
        .unwrap(),
    );
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    storage
        .set_furnace_block_entity(
            position,
            FurnaceBlockEntity {
                burn_remaining: 10,
                burn_total: 10,
                ..FurnaceBlockEntity::default()
            },
        )
        .unwrap();
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(74),
        name: "DurableFurnace".to_string(),
    };
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    sessions.mark_loaded(session_id, (0, 0));
    let config = ServerConfig {
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        ..play_loop_slow_client_test_config()
    };
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    let updated = tokio::time::timeout(
        Duration::from_secs(1),
        owner.run_furnace_ticks(&config, &sessions, Some(&world_read), Some(&world_mutation)),
    )
    .await
    .expect("resident furnace tick completion event");
    assert_eq!(updated, 1);
    assert_eq!(world_read.get_cached_block(position), Some(BlockStateId(2)));
    assert_eq!(
        world_read
            .furnace_snapshots(&[cpos])
            .into_iter()
            .find(|(candidate, _)| *candidate == position)
            .expect("resident furnace")
            .1
            .burn_remaining,
        9
    );
    drop(world_writer);
}

#[tokio::test]
async fn active_furnace_tick_publishes_lit_block_and_light() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            furnace_block(1, 2),
        ])
        .unwrap(),
    );
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    storage
        .set_baked_light(cpos, &mc_world::light::ChunkLight::filled(15, 0))
        .unwrap();
    storage
        .set_furnace_block_entity(
            position,
            FurnaceBlockEntity {
                burn_remaining: 10,
                burn_total: 10,
                ..FurnaceBlockEntity::default()
            },
        )
        .unwrap();
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(75),
        name: "LitFurnace".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(8);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    sessions.mark_loaded(session_id, (0, 0));
    let config = ServerConfig {
        blocks: Arc::clone(&blocks),
        block_light: Some(Arc::new(
            mc_data::block_light::BlockLightTable::from_arrays(
                "furnace lit test",
                vec![0, 0, 13],
                vec![0, 15, 15],
                vec![true, false, false],
            ),
        )),
        world: Some(world),
        ..play_loop_slow_client_test_config()
    };
    let (_simulation, owner) = simulation_channel();

    assert_eq!(
        owner
            .run_furnace_ticks(&config, &sessions, Some(&world_read), Some(&world_mutation))
            .await,
        1
    );
    let commands = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(commands.iter().any(|command| matches!(
        command,
        OutboundCommand::BlockDeltas(deltas)
            if deltas == &[BlockDelta {
                x: position.x,
                y: position.y,
                z: position.z,
                state_id: BlockStateId(2),
            }]
    )));
    assert!(commands.iter().any(|command| matches!(
        command,
        OutboundCommand::LightUpdates(updates)
            if updates.iter().any(|update| update.pos == cpos)
    )));
}

#[test]
fn furnace_tick_block_state_tracks_burning_state() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        furnace_block(1, 2),
    ])
    .unwrap();
    let burning = FurnaceBlockEntity {
        burn_remaining: 1,
        ..FurnaceBlockEntity::default()
    };

    assert_eq!(
        furnace_tick_block_state(&blocks, BlockStateId(1), &burning),
        BlockStateId(2)
    );
    assert_eq!(
        furnace_tick_block_state(&blocks, BlockStateId(2), &FurnaceBlockEntity::default()),
        BlockStateId(1)
    );
}

#[tokio::test]
async fn stale_furnace_tick_wave_replans_against_resident_state() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let current = FurnaceBlockEntity {
        burn_remaining: 9,
        burn_total: 10,
        ..FurnaceBlockEntity::default()
    };
    storage.set_furnace_block_entity(position, current).unwrap();
    let world_read = storage.read_view();
    let mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        blocks: Arc::clone(&blocks),
        world: Some(world),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let stale_before = FurnaceBlockEntity {
        burn_remaining: 10,
        burn_total: 10,
        ..FurnaceBlockEntity::default()
    };
    let mut stale_after = stale_before.clone();
    stale_after.burn_remaining = 9;

    let updates = commit_resident_furnace_tick_wave(
        &config,
        &sessions,
        &mutation,
        vec![FurnaceTickPlan {
            position,
            block_state: BlockStateId(1),
            after_block_state: BlockStateId(1),
            kind: FurnaceKind::Furnace,
            before: stale_before,
            after: stale_after,
            slots_changed: false,
            data_changed: vec![(0, 9)],
        }],
    );
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].1.burn_remaining, 8);
    assert_eq!(
        world_read
            .furnace_snapshots(&[cpos])
            .into_iter()
            .find(|(candidate, _)| *candidate == position)
            .expect("replanned resident furnace")
            .1
            .burn_remaining,
        8
    );
}

#[test]
fn active_furnace_tick_releases_world_writer_between_commits() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let cpos = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    for position in [
        mc_world::BlockPos { x: 1, y: 64, z: 1 },
        mc_world::BlockPos { x: 2, y: 64, z: 1 },
    ] {
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_furnace_block_entity(
                position,
                FurnaceBlockEntity {
                    burn_remaining: 10,
                    burn_total: 10,
                    ..FurnaceBlockEntity::default()
                },
            )
            .unwrap();
    }
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));

    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = Arc::clone(&blocks);
    state.world = Arc::clone(&world);
    state.world_read = world_read.clone();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(73),
        name: "FurnaceWriterBoundary".to_string(),
    };
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = state.sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    state.sessions.mark_loaded(session_id, (0, 0));

    let config = ServerConfig {
        blocks: Arc::clone(&state.blocks),
        world: Some(Arc::clone(&world)),
        tags: Arc::clone(&state.tags),
        recipes: Arc::new(state.recipes.clone()),
        items: Arc::clone(&state.items),
        block_facts: Arc::clone(&state.block_facts),
        ..play_loop_slow_client_test_config()
    };
    let sessions = Arc::clone(&state.sessions);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    sessions.install_server_furnace_commit_probe(reached_tx, resume_rx);

    let tick_sessions = Arc::clone(&sessions);
    let tick_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let (_simulation, owner) = simulation_channel();
                owner
                    .run_furnace_ticks(
                        &config,
                        &tick_sessions,
                        Some(&world_read),
                        Some(&world_mutation),
                    )
                    .await
            })
    });

    reached_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("first furnace commit reaches the exact probe");
    let writer_is_available = world.try_lock().is_ok();
    resume_tx
        .send(())
        .expect("release the exact furnace commit probe");
    assert_eq!(tick_thread.join().expect("furnace tick thread"), 2);
    assert!(
        writer_is_available,
        "furnace tick must release the world writer after each independent commit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_furnace_tick_pushes_to_all_viewers_without_losing_click() {
    let coal = Identifier::parse("minecraft:coal").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: coal,
        protocol_id: 10,
    }]));
    let coal_id = items
        .id_of(&Identifier::parse("minecraft:coal").unwrap())
        .unwrap();
    let mut ticker = interaction_state_for_items(Arc::clone(&items));
    let mut clicker = interaction_state_for_items(items);
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    ticker.blocks = Arc::clone(&blocks);
    ticker.world = Arc::clone(&world);
    ticker.world_read = world_read.clone();
    clicker.blocks = blocks;
    clicker.world = world;
    clicker.world_read = world_read;
    clicker.sessions = Arc::clone(&ticker.sessions);
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = ticker.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let furnace = FurnaceBlockEntity {
            burn_remaining: 10,
            burn_total: 10,
            ..FurnaceBlockEntity::default()
        };
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage.set_furnace_block_entity(position, furnace).unwrap();
    }

    let ticker_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(70),
        name: "FurnaceTicker".to_string(),
    };
    let clicker_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(71),
        name: "FurnaceClicker".to_string(),
    };
    let (ticker_tx, mut ticker_rx) = mpsc::channel(8);
    let (clicker_tx, mut clicker_rx) = mpsc::channel(8);
    let (ticker_id, _) = ticker.sessions.register(
        &ticker_profile,
        (0, 0),
        0,
        HashSet::new(),
        ticker_tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    let (clicker_id, _) = ticker.sessions.register(
        &clicker_profile,
        (0, 0),
        0,
        HashSet::new(),
        clicker_tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    ticker.session_id = ticker_id;
    clicker.session_id = clicker_id;
    ticker.sessions.mark_loaded(ticker_id, (0, 0));
    ticker.sessions.mark_loaded(clicker_id, (0, 0));
    assert_eq!(
        ticker.sessions.register_furnace_viewer(ticker_id, position),
        1
    );
    assert_eq!(
        ticker
            .sessions
            .register_furnace_viewer(clicker_id, position),
        1
    );
    ticker.active_container = Some(ActiveContainer::Furnace(FurnaceWindow::new(
        position,
        7,
        FurnaceKind::Furnace,
    )));
    clicker.carried_item = ItemStack::new(coal_id, 1);

    let config = ServerConfig {
        blocks: Arc::clone(&ticker.blocks),
        world: Some(Arc::clone(&ticker.world)),
        tags: Arc::clone(&ticker.tags),
        recipes: Arc::new(ticker.recipes.clone()),
        items: Arc::clone(&ticker.items),
        block_facts: Arc::clone(&ticker.block_facts),
        ..play_loop_slow_client_test_config()
    };

    let shared_world = Arc::clone(&ticker.world);
    let world_guard = shared_world.lock().await;
    let (_simulation, owner) = simulation_channel();
    let mut tick = Box::pin(owner.run_furnace_ticks(
        &config,
        &ticker.sessions,
        Some(&ticker.world_read),
        Some(&world_mutation),
    ));
    let tick_result =
        std::future::poll_fn(|cx| match std::future::Future::poll(tick.as_mut(), cx) {
            Poll::Ready(updated) => Poll::Ready(updated),
            Poll::Pending => panic!("active furnace tick waited for the world writer"),
        })
        .await;
    assert_eq!(tick_result, 1);
    drop(world_guard);

    let mut clicker_writer = Vec::new();
    let clicker_window = handle_furnace_container_click(
        &mut clicker,
        &mut clicker_writer,
        FurnaceWindow::new(position, 8, FurnaceKind::Furnace),
        PlayerPose::new(0.5, 65.0, 0.5),
        ServerboundContainerClick {
            container_id: 8,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .expect("furnace click succeeds");
    assert_eq!(clicker_window.state_id, 2);
    assert!(clicker.carried_item.is_empty());
    let ticker_commands = std::iter::from_fn(|| ticker_rx.try_recv().ok()).collect::<Vec<_>>();
    let clicker_commands = std::iter::from_fn(|| clicker_rx.try_recv().ok()).collect::<Vec<_>>();
    for commands in [&ticker_commands, &clicker_commands] {
        assert!(commands.iter().any(|command| matches!(
            command,
            OutboundCommand::FurnaceData { position: update_position, changed }
                if *update_position == position && changed.contains(&(0, 9))
        )));
    }
    let mut storage = ticker.world.lock().await;
    let furnace = storage
        .furnace_block_entity(position)
        .unwrap()
        .expect("furnace remains present");
    assert_eq!(furnace.burn_remaining, 9);
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(coal_id, 1),
        "tick data update must not overwrite the queued client slot mutation"
    );
}

#[tokio::test]
async fn owner_furnace_tick_keeps_running_after_window_closes() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = Arc::clone(&blocks);
    state.world = Arc::clone(&world);
    state.world_read = world_read;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_furnace_block_entity(
                position,
                FurnaceBlockEntity {
                    burn_remaining: 10,
                    burn_total: 10,
                    ..FurnaceBlockEntity::default()
                },
            )
            .unwrap();
    }

    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(72),
        name: "FurnaceOwner".to_string(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state.sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    state.sessions.mark_loaded(session_id, (0, 0));

    let config = ServerConfig {
        blocks: Arc::clone(&state.blocks),
        world: Some(Arc::clone(&world)),
        tags: Arc::clone(&state.tags),
        recipes: Arc::new(state.recipes.clone()),
        items: Arc::clone(&state.items),
        block_facts: Arc::clone(&state.block_facts),
        ..play_loop_slow_client_test_config()
    };
    let (_simulation, owner) = simulation_channel();

    assert_eq!(
        owner
            .run_furnace_ticks(
                &config,
                &state.sessions,
                Some(&state.world_read),
                Some(&world_mutation),
            )
            .await,
        1
    );
    let mut storage = world.lock().await;
    let furnace = storage
        .furnace_block_entity(position)
        .unwrap()
        .expect("furnace remains present");
    assert_eq!(furnace.burn_remaining, 9);
}

#[tokio::test]
async fn stale_furnace_click_after_peer_mutation_resyncs_without_mutating_storage() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: stone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let stone_id = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mut furnace = FurnaceBlockEntity::default();
        furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 5));
        storage.set_furnace_block_entity(position, furnace).unwrap();
    }

    let window = FurnaceWindow::new(position, 7, FurnaceKind::Furnace);
    {
        let mut storage = state.world.lock().await;
        let mut furnace = storage
            .furnace_block_entity(position)
            .unwrap()
            .expect("test furnace exists");
        furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(stone_id, 2));
        storage
            .set_furnace_block_entity(position, furnace.clone())
            .unwrap();
        let _ = state.sessions.server_furnace_slot_dispatches_except(
            position,
            99,
            furnace_slot_stacks(&furnace),
        );
    }

    let mut writer = Vec::new();
    let returned = handle_furnace_container_click(
        &mut state,
        &mut writer,
        window,
        PlayerPose::new(0.5, 65.0, 0.5),
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();

    assert_eq!(returned.state_id, 2);
    assert!(state.carried_item.is_empty());
    {
        let mut storage = state.world.lock().await;
        let furnace = storage
            .furnace_block_entity(position)
            .unwrap()
            .expect("test furnace exists");
        assert_eq!(
            furnace_slot_to_stack(&furnace.slots[0]),
            ItemStack::new(stone_id, 2)
        );
    }
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].state_id, 2);
    assert_eq!(packets[0].items[0], ItemStack::new(stone_id, 2));
    assert!(packets[0].carried_item.is_empty());
}
