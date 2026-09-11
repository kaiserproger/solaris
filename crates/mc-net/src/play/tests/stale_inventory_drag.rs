use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use tokio::sync::mpsc;

use super::{
    ContainerClickContext, ContainerInput, GameMode, Identifier, ItemRegistry, ItemReport,
    ItemStack, LoggedInProfile, PlayerPersistedState, PlayerPose, ScriptPlayerId,
    ServerboundContainerClick, SurvivalState, XpState, decode_container_set_content_packets,
    handle_container_click, interaction_state_for_items, no_script_player_context,
    simulation_channel,
};

#[tokio::test]
async fn stale_inventory_drag_resyncs_exact_owner_state_without_loss_or_publication() {
    let item = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.carried_item = ItemStack::new(10, 3);
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StaleInventoryDrag"),
        name: "StaleInventoryDrag".to_owned(),
    };
    let (tx, mut outbound) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.carried_item = state.carried_item.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let sessions = Arc::clone(&state.sessions);
    let (simulation, mut owner) = simulation_channel();
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut writer = Vec::new();
    let xp = XpState::default();
    let carried = mc_protocol::packets::play::HashedStack::Actual {
        item_id: 10,
        count: 3,
        components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
    };
    let script_player_id = ScriptPlayerId::new(state.session_id);
    let script_context = no_script_player_context(state.session_id);

    for (button_num, slot_num) in [(0, -999), (1, 9)] {
        handle_container_click(
            &mut state,
            &mut writer,
            ContainerClickContext {
                game_mode: GameMode::Survival,
                survival_state: SurvivalState::FULL,
                xp_state: &xp,
                player_pose: pose,
                script_events: None,
                scripts: None,
                script_player_id,
                script_context: script_context.clone(),
            },
            ServerboundContainerClick {
                container_id: 0,
                state_id: 1,
                slot_num,
                button_num,
                container_input: ContainerInput::QuickCraft,
                changed_slots: Vec::new(),
                carried_item: carried.clone(),
            },
        )
        .await
        .unwrap();
    }
    assert!(writer.is_empty());

    let mut end = Box::pin(handle_container_click(
        &mut state,
        &mut writer,
        ContainerClickContext {
            game_mode: GameMode::Survival,
            survival_state: SurvivalState::FULL,
            xp_state: &xp,
            player_pose: pose,
            script_events: None,
            scripts: None,
            script_player_id,
            script_context,
        },
        ServerboundContainerClick {
            container_id: 0,
            state_id: 1,
            slot_num: -999,
            button_num: 2,
            container_input: ContainerInput::QuickCraft,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(end.as_mut(), cx).is_pending(),
            "drag must wait for its queued owner commit"
        );
        assert_eq!(simulation_probe.snapshot().depth, 1);
        Poll::Ready(())
    })
    .await;
    {
        let mut saved = saved.lock().unwrap();
        saved.inventory.slots[10] = ItemStack::new(10, 1);
        saved.carried_item = ItemStack::new(10, 2);
    }
    assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
    end.await.unwrap();

    assert!(state.inventory.slots[9].is_empty());
    assert_eq!(state.inventory.slots[10], ItemStack::new(10, 1));
    assert_eq!(state.carried_item, ItemStack::new(10, 2));
    let total = state
        .inventory
        .slots
        .iter()
        .map(|stack| stack.count.max(0))
        .sum::<i32>()
        + state.carried_item.count;
    assert_eq!(total, 3);
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].state_id, 1);
    assert_eq!(packets[0].items, state.inventory.as_wire_list());
    assert_eq!(packets[0].carried_item, state.carried_item);
    assert!(outbound.try_recv().is_err());
    assert!(state.sessions.persisted_entity_records().is_empty());
}

#[tokio::test]
async fn stale_crafting_drag_keeps_all_three_selected_slots() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.carried_item = ItemStack::new(10, 9);
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("QueuedCraftingDrag"),
        name: "QueuedCraftingDrag".into(),
    };
    let (tx, _outbound) = mpsc::channel(32);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.carried_item = state.carried_item.clone();
    state
        .sessions
        .register_player_persistence(session_id, Arc::new(Mutex::new(saved)));
    state.session_id = session_id;
    let sessions = Arc::clone(&state.sessions);
    let (simulation, mut owner) = simulation_channel();
    state.simulation = simulation.for_session(session_id);
    let mut window = Box::new(super::CraftingTableWindow::new(7));
    window.state_id = 2;
    let mut writer = Vec::new();
    for (button_num, slot_num) in [(0, -999), (1, 1), (1, 2), (1, 3), (2, -999)] {
        let carried_item = mc_protocol::packets::play::HashedStack::Actual {
            item_id: 10,
            count: 8,
            components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
        };
        let mut request = Box::pin(super::handle_crafting_container_click(
            &mut state,
            &mut writer,
            window,
            None,
            GameMode::Survival,
            pose,
            ServerboundContainerClick {
                container_id: 7,
                state_id: 1,
                slot_num,
                button_num,
                container_input: ContainerInput::QuickCraft,
                changed_slots: Vec::new(),
                carried_item,
            },
        ));
        window = loop {
            match std::future::poll_fn(|cx| Poll::Ready(request.as_mut().poll(cx))).await {
                Poll::Ready(result) => break result.unwrap(),
                Poll::Pending => assert!(owner.process_tick(&sessions, 32).processed > 0),
            }
        };
    }
    assert_eq!(
        &window.input[..3],
        &[
            ItemStack::new(10, 3),
            ItemStack::new(10, 3),
            ItemStack::new(10, 3)
        ]
    );
    assert!(window.input[3..].iter().all(ItemStack::is_empty));
    assert!(state.carried_item.is_empty());
    let packets = decode_container_set_content_packets(&writer);
    let last = packets.last().expect("authoritative resync");
    assert_eq!(&last.items[1..4], &window.input[..3]);
    assert!(last.carried_item.is_empty());
}
