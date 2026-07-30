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

use crate::play::world_journal::WorldChunkJournal;
use crate::server::ServerConfig;

use super::{
    CAMPFIRE_COOKING_SLOT_COUNT, CAMPFIRE_NBT_COOKING_TIMES, CAMPFIRE_NBT_COOKING_TOTAL_TIMES,
    CampfireCookingState, CampfireCookingTickReport, FurnaceKind, ItemToBlockTable,
    LEGACY_CAMPFIRE_NBT_REMAINING, LEGACY_CAMPFIRE_NBT_TOTAL, PlayerInventory, SessionRegistry,
    campfire_block_entity_persistent_bytes, campfire_block_entity_persistent_nbt,
    campfire_cooking_state_from_persistent_nbt, campfire_test_interaction_state,
    compound_int_array_field, containers, handle_campfire_use_on,
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
    let result = |item: Identifier| RecipeResult { item, count: 1 };
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
    let result = |item: Identifier| RecipeResult { item, count: 1 };
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
        storage.set_opaque_block_entity(pos, bytes).unwrap();
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
    let (journal, pending) = WorldChunkJournal::open(
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
    let (reopened, pending) = WorldChunkJournal::open(
        tmp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
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
