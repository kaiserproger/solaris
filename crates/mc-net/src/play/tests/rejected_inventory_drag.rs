use std::sync::Arc;

use super::{
    ContainerClickContext, ContainerInput, GameMode, Identifier, ItemRegistry, ItemReport,
    ItemStack, PlayerPose, ScriptPlayerId, ServerboundContainerClick, SurvivalState, XpState,
    decode_container_set_content_packets, handle_container_click, interaction_state_for_items,
    no_script_player_context,
};

#[tokio::test]
async fn rejected_inventory_drag_resyncs_without_mutation_or_owner_publication() {
    let item = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.carried_item = ItemStack::new(10, 3);
    let before_inventory = state.inventory.clone();
    let before_carried = state.carried_item.clone();
    let mut writer = Vec::new();
    let carried = || mc_protocol::packets::play::HashedStack::Actual {
        item_id: 10,
        count: 3,
        components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
    };
    let script_player_id = ScriptPlayerId::new(state.session_id);
    let script_context = no_script_player_context(state.session_id);
    let xp = XpState::default();

    for (button_num, slot_num) in [(0, -999), (2, -999)] {
        handle_container_click(
            &mut state,
            &mut writer,
            ContainerClickContext {
                game_mode: GameMode::Survival,
                survival_state: SurvivalState::FULL,
                xp_state: &xp,
                player_pose: PlayerPose::new(0.5, 64.0, 0.5),
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
                carried_item: carried(),
            },
        )
        .await
        .unwrap();
    }

    assert_eq!(state.inventory.slots, before_inventory.slots);
    assert_eq!(state.carried_item, before_carried);
    assert_eq!(state.simulation.snapshot().depth, 0);
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].items, before_inventory.as_wire_list());
    assert_eq!(packets[0].carried_item, before_carried);
}
