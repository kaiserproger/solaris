use std::collections::HashSet;
use std::sync::Arc;

use super::{
    BlockEdit, CommandPermissions, Compression, DebugCommand, DebugCommandContext, Identifier,
    ItemRegistry, ItemReport, ItemStack, PlayerInventory, PlayerPose, SurvivalCommand,
    SurvivalState, XpState, apply_debug_command, debug_water_corridor_edits,
    decode_container_set_slot_packets, interaction_state_for_blocks, interaction_state_for_items,
    parse_debug_command, register_interaction_player, solaris_required_blocks_report,
    spawn_test_simulation_owner,
};

#[test]
fn debug_commands_parse_survival_mutations_and_give() {
    assert_eq!(
        parse_debug_command("debug survival damage 7.5"),
        Some(DebugCommand::Survival(SurvivalCommand::Damage(7.5)))
    );
    assert_eq!(
        parse_debug_command("debug survival heal"),
        Some(DebugCommand::Survival(SurvivalCommand::Heal(20.0)))
    );
    assert_eq!(
        parse_debug_command("debug survival feed 2 0.5"),
        Some(DebugCommand::Survival(SurvivalCommand::Feed {
            food: 2,
            saturation: 0.5
        }))
    );
    assert_eq!(
        parse_debug_command("debug survival exhaust 4"),
        Some(DebugCommand::Survival(SurvivalCommand::Exhaust(4.0)))
    );
    assert_eq!(
        parse_debug_command("debug survival xp 35"),
        Some(DebugCommand::Survival(SurvivalCommand::Experience(35)))
    );
    assert_eq!(
        parse_debug_command("debug give minecraft:dirt 64 1"),
        Some(DebugCommand::Give {
            item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            count: 64,
            hotbar_slot: 1,
        })
    );
    assert_eq!(
        parse_debug_command("debug outbound-pressure 192"),
        Some(DebugCommand::OutboundPressure { count: 192 })
    );
    assert_eq!(
        parse_debug_command("debug water-corridor 4 96 0"),
        Some(DebugCommand::WaterCorridor { x: 4, y: 96, z: 0 })
    );
    assert_eq!(parse_debug_command("debug water-corridor"), None);
    assert_eq!(parse_debug_command("debug water-corridor 4 317 0"), None);
    assert_eq!(
        parse_debug_command("debug water-corridor 4 96 0 extra"),
        None
    );
    assert_eq!(parse_debug_command("debug outbound-pressure 0"), None);
    assert_eq!(parse_debug_command("debug outbound-pressure 257"), None);
    assert_eq!(parse_debug_command("damage 7.5"), None);
    assert_eq!(parse_debug_command("debug survival damage bad"), None);
    assert_eq!(parse_debug_command("debug survival damage NaN"), None);
    assert_eq!(parse_debug_command("debug survival heal inf"), None);
    assert_eq!(parse_debug_command("debug survival feed 2 -inf"), None);
    assert_eq!(parse_debug_command("debug survival exhaust NaN"), None);
}

#[test]
fn debug_water_corridor_fixture_is_closed_unique_and_source_filled() {
    let state = interaction_state_for_blocks(Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap(),
    ));
    let water = state
        .blocks
        .block(&Identifier::parse("minecraft:water").unwrap())
        .expect("fixture registry has water")
        .default;
    let stone = state
        .blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .expect("fixture registry has stone")
        .default;
    let edits = debug_water_corridor_edits(
        &state.blocks,
        Some(water),
        mc_world::BlockPos { x: 4, y: 66, z: 0 },
    )
    .expect("water corridor plan");

    assert_eq!(edits.len(), 68);
    let unique = edits.iter().map(|edit| edit.pos).collect::<HashSet<_>>();
    assert_eq!(unique.len(), edits.len(), "fixture edits must be unique");
    for z in 0..=4 {
        assert!(edits.contains(&BlockEdit {
            pos: mc_world::BlockPos { x: 4, y: 66, z },
            new_state: water,
        }));
        assert!(edits.contains(&BlockEdit {
            pos: mc_world::BlockPos { x: 4, y: 67, z },
            new_state: water,
        }));
        assert!(edits.contains(&BlockEdit {
            pos: mc_world::BlockPos { x: 4, y: 65, z },
            new_state: stone,
        }));
    }
}

#[tokio::test]
async fn debug_give_zero_count_clears_hotbar_slot_before_item_lookup() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(10, 1);
    let session_id = register_interaction_player(&mut state, "DebugGiveClear");
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);
    let mut writer = Vec::new();
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();

    apply_debug_command(
        &mut writer,
        Compression::Disabled,
        DebugCommand::Give {
            item: Identifier::parse("minecraft:air").unwrap(),
            count: 0,
            hotbar_slot: 0,
        },
        DebugCommandContext {
            survival_state: &mut survival_state,
            xp_state: &mut xp_state,
            interaction: Some(&mut state),
            player_pose: PlayerPose::new(0.0, 64.0, 0.0),
            permissions: CommandPermissions { op: true },
        },
    )
    .await
    .unwrap();

    stop.send(()).unwrap();
    task.await.unwrap();

    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::EMPTY
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].item_stack, ItemStack::EMPTY);
}
