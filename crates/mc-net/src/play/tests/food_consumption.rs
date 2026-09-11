use super::*;

#[tokio::test]
async fn embedded_food_consumption_preserves_timing_and_hunger_eligibility() {
    for (name, nutrition, saturation, initial_food) in [
        ("minecraft:cooked_cod", 5, 6.0_f32, 1),
        ("minecraft:cooked_salmon", 6, 9.6, 1),
        ("minecraft:cooked_mutton", 6, 9.6, 1),
        ("minecraft:golden_apple", 4, 9.6, 20),
        ("minecraft:chorus_fruit", 4, 2.4, 20),
    ] {
        let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
            id: Identifier::parse(name).unwrap(),
            protocol_id: 10,
        }]));
        let mut state = interaction_state_for_items(items);
        state.item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
        state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(10, 2);
        let pose = PlayerPose::new(0.5, 64.0, 0.5);
        let profile = LoggedInProfile {
            uuid: crate::login::offline_uuid("FoodConsumption"),
            name: "FoodConsumption".into(),
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let (session_id, _) =
            state
                .sessions
                .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
        state.session_id = session_id;
        let mut survival = SurvivalState {
            food: initial_food,
            saturation: 0.0,
            ..SurvivalState::FULL
        };
        let mut persisted = PlayerPersistedState::new_default(pose);
        persisted.inventory = state.inventory.clone();
        persisted.survival = survival;
        state.player_persistence = Arc::new(std::sync::Mutex::new(persisted));
        state
            .sessions
            .register_player_persistence(session_id, Arc::clone(&state.player_persistence));
        let (simulation, mut owner) = simulation_channel();
        state.simulation = simulation.for_session(session_id);
        let sessions = Arc::clone(&state.sessions);
        let mut wire = Vec::new();
        handle_use_item(
            &mut state,
            &mut wire,
            GameMode::Survival,
            &mut survival,
            pose,
            ServerboundUseItem {
                hand: InteractionHand::MainHand,
                sequence: 1,
                y_rot: 0.0,
                x_rot: 0.0,
            },
        )
        .await
        .unwrap();
        let pending = state
            .pending_use
            .expect("ordinary cooked food starts consumption");
        assert_eq!(pending.required_ticks, 32, "{name}");
        tick_pending_use(&mut state, &mut wire, GameMode::Survival, &mut survival, 31)
            .await
            .unwrap();
        assert_eq!(state.inventory.slots[PlayerInventory::HOTBAR_BASE].count, 2);
        let mut finish = Box::pin(tick_pending_use(
            &mut state,
            &mut wire,
            GameMode::Survival,
            &mut survival,
            32,
        ));
        loop {
            match std::future::poll_fn(|cx| Poll::Ready(finish.as_mut().poll(cx))).await {
                Poll::Ready(result) => {
                    result.unwrap();
                    break;
                }
                Poll::Pending => assert!(owner.process_tick(&sessions, 32).processed > 0),
            }
        }
        drop(finish);
        assert_eq!(
            state.inventory.slots[PlayerInventory::HOTBAR_BASE].count,
            1,
            "{name}"
        );
        assert_eq!(survival.food, (initial_food + nutrition).min(20), "{name}");
        assert_eq!(
            survival.saturation,
            saturation.min((initial_food + nutrition).min(20) as f32),
            "{name}"
        );
        assert_eq!(
            crate::lock_policy::lock_authoritative_mutex(
                &state.player_persistence,
                "food test persistence"
            )
            .survival,
            survival
        );
    }
}

#[test]
fn food_holder_without_consumable_component_cannot_start_eating() {
    let facts = mc_data::item_components::solaris_required_item_facts();
    for name in [
        "minecraft:cod_bucket",
        "minecraft:salmon_bucket",
        "minecraft:pufferfish_bucket",
        "minecraft:tropical_fish_bucket",
    ] {
        let item = Identifier::parse(name).unwrap();
        assert!(
            mc_data::food::rule_for_item(&facts, &item, Duration::from_millis(1600)).is_none(),
            "{name}"
        );
    }
}

#[tokio::test]
async fn ordinary_food_cannot_start_at_full_hunger() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:cooked_cod").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(10, 1);
    let mut survival = SurvivalState::FULL;
    handle_use_item(
        &mut state,
        &mut Vec::new(),
        GameMode::Survival,
        &mut survival,
        PlayerPose::new(0.5, 64.0, 0.5),
        ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 1,
            y_rot: 0.0,
            x_rot: 0.0,
        },
    )
    .await
    .unwrap();
    assert!(state.pending_use.is_none());
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(10, 1)
    );
}

#[tokio::test]
async fn consumption_conserves_remainders_in_hand_inventory_and_overflow() {
    for (food, held_count, full, merge, remainder_id, ticks) in [
        ("minecraft:mushroom_stew", 1, false, false, 11, 32),
        ("minecraft:honey_bottle", 2, false, false, 12, 40),
        ("minecraft:honey_bottle", 2, true, true, 12, 40),
        ("minecraft:honey_bottle", 2, true, false, 12, 40),
    ] {
        let items = Arc::new(ItemRegistry::from_report(&[
            ItemReport {
                id: Identifier::parse(food).unwrap(),
                protocol_id: 10,
            },
            ItemReport {
                id: Identifier::parse("minecraft:bowl").unwrap(),
                protocol_id: 11,
            },
            ItemReport {
                id: Identifier::parse("minecraft:glass_bottle").unwrap(),
                protocol_id: 12,
            },
            ItemReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 13,
            },
        ]));
        let mut state = interaction_state_for_items(items);
        state.item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
        state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
        if full {
            state.inventory.slots[9..=44].fill(ItemStack::new(13, 64));
        }
        if merge {
            state.inventory.slots[9] = ItemStack::new(remainder_id, 63);
        }
        let slot = PlayerInventory::HOTBAR_BASE;
        state.inventory.slots[slot] = ItemStack::new(10, held_count);
        let pose = PlayerPose::new(0.5, 64.0, 0.5);
        let profile = LoggedInProfile {
            uuid: crate::login::offline_uuid("FoodRemainder"),
            name: "FoodRemainder".into(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let (session_id, _) =
            state
                .sessions
                .register(&profile, (0, 0), 0, HashSet::from([(0, 0)]), tx, pose);
        state.session_id = session_id;
        let _ = state.sessions.mark_loaded(session_id, (0, 0));
        let mut survival = SurvivalState {
            food: 1,
            saturation: 0.0,
            ..SurvivalState::FULL
        };
        let mut persisted = PlayerPersistedState::new_default(pose);
        persisted.inventory = state.inventory.clone();
        persisted.survival = survival;
        state.player_persistence = Arc::new(std::sync::Mutex::new(persisted));
        state
            .sessions
            .register_player_persistence(session_id, Arc::clone(&state.player_persistence));
        let (simulation, mut owner) = simulation_channel();
        state.simulation = simulation.for_session(session_id);
        let sessions = Arc::clone(&state.sessions);
        let mut wire = Vec::new();
        handle_use_item(
            &mut state,
            &mut wire,
            GameMode::Survival,
            &mut survival,
            pose,
            ServerboundUseItem {
                hand: InteractionHand::MainHand,
                sequence: 1,
                y_rot: 0.0,
                x_rot: 0.0,
            },
        )
        .await
        .unwrap();
        assert_eq!(state.pending_use.unwrap().required_ticks, ticks);
        let mut finish = Box::pin(tick_pending_use(
            &mut state,
            &mut wire,
            GameMode::Survival,
            &mut survival,
            ticks,
        ));
        loop {
            match std::future::poll_fn(|cx| Poll::Ready(finish.as_mut().poll(cx))).await {
                Poll::Ready(result) => {
                    result.unwrap();
                    break;
                }
                Poll::Pending => assert!(owner.process_tick(&sessions, 1).processed > 0),
            }
        }
        drop(finish);
        let inventory_remainders: i32 = state
            .inventory
            .slots
            .iter()
            .filter(|stack| !stack.is_empty() && stack.item_id == remainder_id)
            .map(|stack| stack.count)
            .sum();
        let mut dropped_remainders = 0;
        while let Ok(command) = rx.try_recv() {
            if let OutboundCommand::SpawnEntity(entity) = command
                && let Some(stack) = entity.item_stack
            {
                assert_eq!(stack.item_id, remainder_id);
                dropped_remainders += stack.count;
            }
        }
        assert_eq!(
            inventory_remainders + dropped_remainders,
            if merge { 64 } else { 1 },
            "{food} full={full} merge={merge} held={:?}",
            state.inventory.slots[slot]
        );
        assert_eq!(dropped_remainders, i32::from(full && !merge));
        if held_count == 1 {
            assert_eq!(state.inventory.slots[slot], ItemStack::new(remainder_id, 1));
        } else {
            assert_eq!(
                state.inventory.slots[slot],
                ItemStack::new(10, held_count - 1)
            );
        }
        assert_eq!(
            crate::lock_policy::lock_authoritative_mutex(
                &state.player_persistence,
                "remainder test"
            )
            .inventory
            .slots,
            state.inventory.slots,
        );
    }
}
