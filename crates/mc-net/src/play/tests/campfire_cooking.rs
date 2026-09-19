use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use mc_data::Identifier;
use mc_data::blocks::BlockReport;
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::tags::TagsData;
use mc_entity::EntityItemStack;
use mc_nbt::{ListTag, Tag};
use mc_protocol::packets::play::{BlockChangedAck, GameMode, InteractionHand, ItemStack};
use mc_protocol::{Compression, Packet};
use mc_world::{BlockStateId, Chunk, ChunkPos};
use tokio::sync::mpsc;

use crate::login::LoggedInProfile;
use crate::play::world_journal::WorldChunkJournal;
use crate::server::ServerConfig;

use super::{
    BlockDelta, BlockEdit, CAMPFIRE_COOKING_SLOT_COUNT, CAMPFIRE_NBT_COOKING_TIMES,
    CAMPFIRE_NBT_COOKING_TOTAL_TIMES, CampfireCookingState, CampfireCookingTickReport, FurnaceKind,
    ItemToBlockTable, LEGACY_CAMPFIRE_NBT_REMAINING, LEGACY_CAMPFIRE_NBT_TOTAL, OutboundCommand,
    PlayerInventory, PlayerPose, SessionRegistry, SimulationRequestError,
    campfire_block_entity_persistent_bytes, campfire_block_entity_persistent_nbt,
    campfire_cooking_state_from_persistent_nbt, campfire_test_interaction_state,
    compound_int_array_field, containers, dispatch_and_clear_setup_packets, handle_campfire_use_on,
    hydrate_persisted_campfire_cooking, play_loop_slow_client_test_config, prop_schema,
    simple_block, simulation_channel, state,
};

#[test]
fn furnace_like_recipe_lookup_uses_matching_cooking_category() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let iron_ore = Identifier::parse("minecraft:iron_ore").unwrap();
    let raw_iron = Identifier::parse("minecraft:raw_iron").unwrap();
    let beef = Identifier::parse("minecraft:beef").unwrap();
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let cooked_beef = Identifier::parse("minecraft:cooked_beef").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: iron_ore.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: raw_iron.clone(),
            protocol_id: 11,
        },
        ItemReport {
            id: beef.clone(),
            protocol_id: 12,
        },
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: iron_ingot.clone(),
            protocol_id: 20,
        },
        ItemReport {
            id: cooked_beef.clone(),
            protocol_id: 21,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]);
    let ingredient = |item: Identifier| Ingredient {
        alternatives: vec![IngredientAlternative::Item(item)],
    };
    let cooking = |item: Identifier, cooking_time| SmeltingRecipe {
        ingredient: ingredient(item),
        cooking_time,
        experience_milli: 0,
    };
    let result = |item: Identifier| RecipeResult {
        item,
        count: 1,
        stew_effects: Vec::new(),
    };
    let recipes = vec![
        Recipe {
            id: Identifier::parse("minecraft:test_smelting").unwrap(),
            kind: RecipeKind::Smelting(cooking(iron_ore, 200)),
            result: result(iron_ingot.clone()),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_blasting").unwrap(),
            kind: RecipeKind::Blasting(cooking(raw_iron.clone(), 100)),
            result: result(iron_ingot),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_smoking").unwrap(),
            kind: RecipeKind::Smoking(cooking(beef.clone(), 100)),
            result: result(cooked_beef),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_campfire").unwrap(),
            kind: RecipeKind::CampfireCooking(cooking(porkchop.clone(), 600)),
            result: result(cooked_porkchop),
        },
    ];
    let tags = TagsData::default();

    assert_eq!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 10)
            .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_smelting").unwrap())
    );
    assert!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 11)
            .is_none()
    );
    assert_eq!(
        containers::find_cooking_recipe_for_item(
            &recipes,
            &items,
            &tags,
            FurnaceKind::BlastFurnace,
            11
        )
        .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_blasting").unwrap())
    );
    assert_eq!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Smoker, 12)
            .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_smoking").unwrap())
    );
    assert!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 13)
            .is_none()
    );
}

#[test]
fn campfire_recipe_lookup_uses_campfire_category() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let beef = Identifier::parse("minecraft:beef").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let cooked_beef = Identifier::parse("minecraft:cooked_beef").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: beef.clone(),
            protocol_id: 14,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
        ItemReport {
            id: cooked_beef.clone(),
            protocol_id: 23,
        },
    ]);
    let ingredient = |item: Identifier| Ingredient {
        alternatives: vec![IngredientAlternative::Item(item)],
    };
    let cooking = |item: Identifier, cooking_time| SmeltingRecipe {
        ingredient: ingredient(item),
        cooking_time,
        experience_milli: 0,
    };
    let result = |item: Identifier| RecipeResult {
        item,
        count: 1,
        stew_effects: Vec::new(),
    };
    let recipes = vec![
        Recipe {
            id: Identifier::parse("minecraft:test_smoking").unwrap(),
            kind: RecipeKind::Smoking(cooking(beef.clone(), 100)),
            result: result(cooked_beef),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_campfire").unwrap(),
            kind: RecipeKind::CampfireCooking(cooking(porkchop, 600)),
            result: result(cooked_porkchop),
        },
    ];
    let tags = TagsData::default();

    assert_eq!(
        containers::find_campfire_recipe_in(&recipes, &items, &tags, 13).map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_campfire").unwrap())
    );
    assert!(containers::find_campfire_recipe_in(&recipes, &items, &tags, 14).is_none());
}

#[test]
fn campfire_cooking_rejects_invalid_when_full() {
    let mut cooking = CampfireCookingState::default();

    for item_id in 1..=CAMPFIRE_COOKING_SLOT_COUNT as u32 {
        assert!(cooking.insert(ItemStack::new(item_id, 1), ItemStack::new(item_id, 1), 5));
    }
    assert!(!cooking.insert(ItemStack::new(99, 1), ItemStack::new(99, 1), 5));
}

#[test]
fn unlit_campfire_cools_every_active_slot_by_two_progress() {
    let mut cooking = CampfireCookingState::default();
    for item_id in 1..=CAMPFIRE_COOKING_SLOT_COUNT as u32 {
        assert!(cooking.insert(
            ItemStack::new(item_id, 1),
            ItemStack::new(item_id + 10, 1),
            10
        ));
    }
    cooking.slots[0].as_mut().unwrap().ticks_remaining = 9;
    cooking.slots[1].as_mut().unwrap().ticks_remaining = 7;
    cooking.slots[2].as_mut().unwrap().ticks_remaining = 10;
    cooking.slots[3].as_mut().unwrap().ticks_remaining = 1;

    assert!(cooking.cool_down());
    assert_eq!(
        cooking
            .slots
            .each_ref()
            .map(|slot| slot.as_ref().unwrap().ticks_remaining),
        [10, 9, 10, 3]
    );
}

#[tokio::test]
async fn full_campfire_consumes_valid_food_interaction_without_debit() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let position = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    let mut state = campfire_test_interaction_state(position).await;
    let raw = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    state.items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: raw.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: cooked.clone(),
            protocol_id: 22,
        },
    ]));
    state.item_to_block = ItemToBlockTable::build(&state.items, &state.blocks);
    state.recipes = vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(raw)],
            },
            cooking_time: 100,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked,
            count: 1,
            stew_effects: Vec::new(),
        },
    }];
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(13, 5);
    for _ in 0..CAMPFIRE_COOKING_SLOT_COUNT {
        assert!(
            state
                .sessions
                .insert_campfire_cooking(
                    position,
                    ItemStack::new(13, 1),
                    ItemStack::new(22, 1),
                    100,
                )
                .is_some()
        );
    }
    let expected = state.sessions.campfire_cooking_state(position);
    let mut writer = Vec::new();

    assert!(
        handle_campfire_use_on(
            &mut state,
            &mut writer,
            GameMode::Survival,
            77,
            position,
            InteractionHand::MainHand,
        )
        .await
        .unwrap()
    );
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(13, 5)
    );
    assert_eq!(state.sessions.campfire_cooking_state(position), expected);
    let mut bytes = bytes::BytesMut::from(writer.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut bytes, Compression::Disabled)
        .unwrap()
        .expect("full campfire interaction acknowledgement");
    assert_eq!(frame.id, BlockChangedAck::ID);
    assert_eq!(
        BlockChangedAck::decode(&mut frame.body).unwrap().sequence,
        77
    );
    assert!(bytes.is_empty());
}

#[test]
fn campfire_cooking_moves_completed_output_to_pending_intent() {
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(41, 1), ItemStack::new(42, 1), 2));

    assert!(cooking.tick().completed.is_empty());
    assert_eq!(cooking.tick().completed, vec![ItemStack::new(42, 1)]);
    assert!(cooking.slots.iter().all(Option::is_none));
    assert_eq!(cooking.pending_outputs.len(), 1);
    assert_eq!(
        cooking.pending_outputs[0].stack,
        EntityItemStack::new(42, 1)
    );
}

#[test]
fn campfire_persistent_nbt_uses_vanilla_cooking_arrays_and_reads_legacy() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]);
    let recipes = vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(porkchop.clone())],
            },
            cooking_time: 100,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_porkchop,
            count: 1,
            stew_effects: Vec::new(),
        },
    }];
    let tags = TagsData::default();

    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 100));
    cooking.slots[0].as_mut().unwrap().ticks_remaining = 75;
    let tag = campfire_block_entity_persistent_nbt(
        "minecraft:campfire",
        mc_world::BlockPos { x: 1, y: 2, z: 3 },
        &items,
        &cooking,
    )
    .expect("persistent campfire tag");
    assert_eq!(
        compound_int_array_field(&tag, CAMPFIRE_NBT_COOKING_TIMES),
        Some(&[25, 0, 0, 0][..])
    );
    assert_eq!(
        compound_int_array_field(&tag, CAMPFIRE_NBT_COOKING_TOTAL_TIMES),
        Some(&[100, 0, 0, 0][..])
    );
    assert_eq!(
        compound_int_array_field(&tag, LEGACY_CAMPFIRE_NBT_REMAINING),
        None
    );

    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &tag).expect("encode vanilla campfire tag");
    let restored =
        campfire_cooking_state_from_persistent_nbt(&bytes, &recipes, &items, &tags).unwrap();
    let restored_slot = restored.slots[0].as_ref().unwrap();
    assert_eq!(restored_slot.ticks_remaining, 75);
    assert_eq!(restored_slot.cooking_time_total, 100);

    let legacy_tag = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:campfire".into())),
        (
            "Items".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("Slot".into(), Tag::Int(0)),
                    ("id".into(), Tag::String(porkchop.as_str().to_string())),
                    ("count".into(), Tag::Int(1)),
                ])],
            }),
        ),
        (
            LEGACY_CAMPFIRE_NBT_REMAINING.into(),
            Tag::IntArray(vec![33, 0, 0, 0]),
        ),
        (
            LEGACY_CAMPFIRE_NBT_TOTAL.into(),
            Tag::IntArray(vec![100, 0, 0, 0]),
        ),
    ]);
    let mut legacy_bytes = Vec::new();
    mc_nbt::write_network(&mut legacy_bytes, &legacy_tag).expect("encode legacy campfire tag");
    let restored_legacy =
        campfire_cooking_state_from_persistent_nbt(&legacy_bytes, &recipes, &items, &tags).unwrap();
    let restored_legacy_slot = restored_legacy.slots[0].as_ref().unwrap();
    assert_eq!(restored_legacy_slot.ticks_remaining, 33);
    assert_eq!(restored_legacy_slot.cooking_time_total, 100);
}

#[tokio::test]
async fn campfire_startup_hydration_only_reads_resident_chunks() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:campfire"),
        ])
        .unwrap(),
    );
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]));
    let recipes = Arc::new(vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 100,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_porkchop,
            count: 1,
            stew_effects: Vec::new(),
        },
    }]);
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let cpos = ChunkPos { x: 0, z: 0 };
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 100));
    let bytes = campfire_block_entity_persistent_bytes("minecraft:campfire", pos, &items, &cooking)
        .expect("campfire persistence bytes");
    {
        let mut storage =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 8)
                .unwrap()
                .with_item_registry(Arc::clone(&items));
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
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
        let token = storage.block_mutation_token(pos).unwrap();
        assert!(
            storage
                .commit_opaque_block_entity_conditionally(pos, BlockStateId(1), token, bytes)
                .unwrap()
        );
        assert_eq!(storage.flush_dirty().unwrap(), 1);
    }

    let world = Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 8)
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(TagsData::default()),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();

    assert_eq!(
        hydrate_persisted_campfire_cooking(&config, &sessions).await,
        0
    );
    assert!(sessions.campfire_cooking_state(pos).is_empty());

    world
        .lock()
        .await
        .get_chunk_without_generation(cpos)
        .unwrap()
        .expect("load persisted campfire chunk");
    assert_eq!(
        hydrate_persisted_campfire_cooking(&config, &sessions).await,
        1
    );
    assert!(!sessions.campfire_cooking_state(pos).is_empty());
}

#[tokio::test]
async fn campfire_tick_does_not_load_cold_chunks_and_is_durable_when_resident() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:campfire").unwrap(),
                properties: prop_schema(&[("lit", &["true"])]),
                states: vec![state(1, true, &[("lit", "true")])],
            },
        ])
        .unwrap(),
    );
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let second_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let cpos = ChunkPos { x: 0, z: 0 };
    {
        let mut storage =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 8)
                .unwrap()
                .with_item_registry(Arc::clone(&items));
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
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
        storage.set_block_at(second_pos, BlockStateId(1)).unwrap();
        assert_eq!(storage.flush_dirty().unwrap(), 1);
    }

    let world = Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 8)
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        recipes: Arc::new(vec![Recipe {
            id: Identifier::parse("minecraft:test_campfire").unwrap(),
            kind: RecipeKind::CampfireCooking(SmeltingRecipe {
                ingredient: Ingredient {
                    alternatives: vec![IngredientAlternative::Item(porkchop)],
                },
                cooking_time: 2,
                experience_milli: 0,
            }),
            result: RecipeResult {
                item: cooked_porkchop,
                count: 1,
                stew_effects: Vec::new(),
            },
        }]),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 2));
    assert!(sessions.restore_campfire_cooking(pos, cooking));
    let (_simulation, owner) = simulation_channel();

    assert!(world.lock().await.cached_chunk_snapshot(cpos).is_none());
    assert_eq!(
        owner
            .run_campfire_cooking_ticks(&config, &sessions, None, None)
            .await,
        CampfireCookingTickReport::default()
    );
    assert!(world.lock().await.cached_chunk_snapshot(cpos).is_none());
    assert_eq!(
        sessions.campfire_cooking_state(pos).slots[0]
            .as_ref()
            .unwrap()
            .ticks_remaining,
        2
    );

    world
        .lock()
        .await
        .get_chunk_without_generation(cpos)
        .unwrap()
        .expect("load persisted campfire chunk");
    assert!(sessions.restore_campfire_cooking(second_pos, sessions.campfire_cooking_state(pos),));
    let (world_read, world_mutation) = {
        let storage = world.lock().await;
        (storage.read_view(), storage.mutation_view())
    };
    let (journal, pending) = WorldChunkJournal::open_for_test(
        tmp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(1),
        owner.run_campfire_cooking_ticks(
            &config,
            &sessions,
            Some(&world_read),
            Some(&world_mutation),
        ),
    )
    .await
    .expect("resident campfire journal completion event");
    assert_eq!(
        report,
        CampfireCookingTickReport {
            persisted: 2,
            completed: 0,
            dropped: 0,
        }
    );
    assert_eq!(
        sessions.campfire_cooking_state(pos).slots[0]
            .as_ref()
            .unwrap()
            .ticks_remaining,
        1
    );
    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    assert_eq!(pending.len(), 1, "one campfire pass uses one WAL decision");
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    for position in [pos, second_pos] {
        let bytes = restored[0]
            .block_entities
            .get(&position)
            .expect("journaled campfire block entity");
        let cooking = campfire_cooking_state_from_persistent_nbt(
            bytes,
            &config.recipes,
            &config.items,
            &config.tags,
        )
        .expect("journaled campfire cooking state");
        assert_eq!(cooking.slots[0].as_ref().unwrap().ticks_remaining, 1);
    }
    drop(writer);
}

/// Server-owned batches (settlement structure placement) must reach the
/// authoritative world through the simulation pipeline: one command carries the
/// whole batch, the cross-region edits commit, and a replaced campfire's cooking
/// state is evicted because no writer session runs the visible-edit finalize
/// pass.
#[tokio::test]
async fn server_owned_block_edits_commit_one_batch_and_evict_replaced_campfire_cooking() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:campfire"),
            simple_block(2, "minecraft:oak_planks"),
        ])
        .unwrap(),
    );
    let campfire_pos = mc_world::BlockPos { x: 3, y: 70, z: 4 };
    let planks_pos = mc_world::BlockPos {
        x: 131,
        y: 70,
        z: 4,
    };
    let world = Arc::new(tokio::sync::Mutex::new({
        let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
        for chunk in [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }] {
            storage
                .insert_generated_chunk(
                    chunk,
                    Chunk::empty(
                        chunk,
                        BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
        }
        storage.set_block_at(campfire_pos, BlockStateId(1)).unwrap();
        storage
    }));
    let sessions = SessionRegistry::new();
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 100));
    assert!(sessions.restore_campfire_cooking(campfire_pos, cooking));
    assert!(!sessions.campfire_cooking_state(campfire_pos).is_empty());

    let (handle, mut owner) = simulation_channel();
    let mut request = Box::pin(handle.apply_server_owned_block_edits(
        "test",
        vec![
            BlockEdit {
                pos: campfire_pos,
                new_state: BlockStateId(2),
            },
            BlockEdit {
                pos: planks_pos,
                new_state: BlockStateId(2),
            },
        ],
        None,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            Future::poll(request.as_mut(), cx).is_pending(),
            "request must wait for the simulation owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 1, "request must be enqueued");

    assert_eq!(
        owner
            .process_commands_with_world(&sessions, Some(&world), None, 1)
            .await
            .processed,
        1,
        "the whole server-owned portion is one command"
    );

    let outcome = request
        .await
        .expect("server-owned batch response")
        .expect("server-owned batch applied");
    assert_eq!(outcome.applied.len(), 2);
    let world = world.lock().await;
    assert_eq!(
        world.get_cached_block(campfire_pos),
        Some(BlockStateId(2)),
        "the replaced campfire is committed"
    );
    assert_eq!(
        world.get_cached_block(planks_pos),
        Some(BlockStateId(2)),
        "the same batch commits its cross-region edit"
    );
    assert!(
        sessions.campfire_cooking_state(campfire_pos).is_empty(),
        "a replaced campfire must not keep cooking state"
    );
}

/// A session-fenced handle must be refused before anything is enqueued, so a
/// fenced caller cannot mutate the world or evict cooking state.
#[tokio::test]
async fn server_owned_block_edits_reject_fenced_handle_without_mutation() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:campfire"),
            simple_block(2, "minecraft:oak_planks"),
        ])
        .unwrap(),
    );
    let campfire_pos = mc_world::BlockPos { x: 3, y: 70, z: 4 };
    let world = Arc::new(tokio::sync::Mutex::new({
        let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(campfire_pos, BlockStateId(1)).unwrap();
        storage
    }));
    let sessions = SessionRegistry::new();
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 100));
    assert!(sessions.restore_campfire_cooking(campfire_pos, cooking));

    let (handle, mut owner) = simulation_channel();
    let fenced = handle.for_session(1);
    assert_eq!(
        fenced
            .apply_server_owned_block_edits(
                "test",
                vec![BlockEdit {
                    pos: campfire_pos,
                    new_state: BlockStateId(2),
                }],
                None,
            )
            .await
            .unwrap_err(),
        SimulationRequestError::InvalidCommand
    );
    assert_eq!(handle.snapshot().depth, 0, "nothing may be enqueued");
    assert_eq!(
        owner
            .process_commands_with_world(&sessions, Some(&world), None, 1)
            .await
            .processed,
        0
    );
    assert_eq!(
        world.lock().await.get_cached_block(campfire_pos),
        Some(BlockStateId(1)),
        "the world must be unchanged"
    );
    assert!(
        !sessions.campfire_cooking_state(campfire_pos).is_empty(),
        "cooking state must survive a refused request"
    );
}

/// A single-region, fully cached server-owned batch must take the same staged
/// path as a cross-region one: a session fast lane skips the eviction a
/// server-owned batch owes, so a replaced campfire would keep cooking state.
#[tokio::test]
async fn server_owned_block_edits_evict_cooking_on_a_single_region_batch() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:campfire"),
            simple_block(2, "minecraft:oak_planks"),
        ])
        .unwrap(),
    );
    let campfire_pos = mc_world::BlockPos { x: 3, y: 70, z: 4 };
    let world = Arc::new(tokio::sync::Mutex::new({
        let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(campfire_pos, BlockStateId(1)).unwrap();
        storage
    }));
    let sessions = SessionRegistry::new();
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 100));
    assert!(sessions.restore_campfire_cooking(campfire_pos, cooking));
    assert!(!sessions.campfire_cooking_state(campfire_pos).is_empty());

    let (handle, mut owner) = simulation_channel();
    let mut request = Box::pin(handle.apply_server_owned_block_edits(
        "test",
        vec![BlockEdit {
            pos: campfire_pos,
            new_state: BlockStateId(2),
        }],
        None,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            Future::poll(request.as_mut(), cx).is_pending(),
            "request must wait for the simulation owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 1, "request must be enqueued");

    assert_eq!(
        owner
            .process_commands_with_world(&sessions, Some(&world), None, 1)
            .await
            .processed,
        1,
        "a single-region server-owned batch is still one command"
    );

    let outcome = request
        .await
        .expect("server-owned batch response")
        .expect("server-owned batch applied");
    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(
        world.lock().await.get_cached_block(campfire_pos),
        Some(BlockStateId(2)),
        "the replaced campfire is committed"
    );
    assert!(
        sessions.campfire_cooking_state(campfire_pos).is_empty(),
        "a single-region server-owned batch must evict replaced campfire cooking"
    );
}

/// A committed server-owned batch must also be *published*: the simulation
/// pipeline's post-commit fanout is the only path that turns settlement
/// structure placement into a client-visible `BlockDeltas` packet, so a loaded
/// session in the edited chunk has to observe the delta, not just storage.
#[tokio::test]
async fn server_owned_block_edits_publish_deltas_to_a_loaded_session() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:campfire"),
            simple_block(2, "minecraft:oak_planks"),
        ])
        .unwrap(),
    );
    let target = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    // A same-chunk position the batch never names: the negative half of the
    // assertion below proves the receiver sees exactly the enqueued edit, not
    // a blanket chunk update.
    let never_enqueued = mc_world::BlockPos { x: 2, y: 64, z: 2 };
    let world = Arc::new(tokio::sync::Mutex::new({
        let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(target, BlockStateId(0)).unwrap();
        storage
            .set_block_at(never_enqueued, BlockStateId(0))
            .unwrap();
        storage
    }));

    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(0x5042),
        name: "ServerOwnedPublication".to_owned(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    dispatch_and_clear_setup_packets(sessions.mark_loaded(session_id, (0, 0)), &mut [&mut rx]);

    let (handle, mut owner) = simulation_channel();
    let mut request = Box::pin(handle.apply_server_owned_block_edits(
        "test",
        vec![BlockEdit {
            pos: target,
            new_state: BlockStateId(2),
        }],
        None,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            Future::poll(request.as_mut(), cx).is_pending(),
            "request must wait for the simulation owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 1, "request must be enqueued");

    assert_eq!(
        owner
            .process_commands_with_world(&sessions, Some(&world), None, 1)
            .await
            .processed,
        1,
        "the whole server-owned portion is one command"
    );

    let outcome = request
        .await
        .expect("server-owned batch response")
        .expect("server-owned batch applied");
    assert_eq!(outcome.applied.len(), 1, "the batch must be committed");
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(2)),
        "the enqueued edit is committed to storage"
    );

    let commands = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    let deltas = commands
        .iter()
        .filter_map(|command| match command {
            OutboundCommand::BlockDeltas(deltas) => Some(deltas.as_slice()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<&BlockDelta>>();
    assert!(
        deltas.iter().any(|delta| delta.x == target.x
            && delta.y == target.y
            && delta.z == target.z
            && delta.state_id == BlockStateId(2)),
        "the loaded session must observe the committed edit, got {commands:?}"
    );
    assert!(
        deltas
            .iter()
            .all(|delta| (delta.x, delta.y, delta.z) == (target.x, target.y, target.z)),
        "no delta may name a position the batch never enqueued ({never_enqueued:?}), got {commands:?}"
    );
}
