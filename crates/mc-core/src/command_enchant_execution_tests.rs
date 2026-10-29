//! The existing dispatcher is the test surface before and after the pilot.
use crate::boundary::{ChangeSet, ConnectionId, EntityId, PlayerChange, PlayerInput};
use crate::command_execution::{CommandResult, execute};
use crate::command_feedback::{text, translated};
use crate::commands::parse;
use crate::tick::{TickState, run_tick};
use mc_data::{Identifier, ItemStack};
use std::sync::Arc;

fn fixture() -> (TickState, EntityId, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 0, 512);
    run_tick(
        &mut state,
        &[
            PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::new_v4(),
                name: "Alice".into(),
                permission_level: 2,
            },
            PlayerInput::Join {
                connection: ConnectionId(2),
                uuid: uuid::Uuid::new_v4(),
                name: "Bob".into(),
                permission_level: 2,
            },
        ],
    );
    let alice = state.connections.entity_of(ConnectionId(1)).unwrap();
    let bob = state.connections.entity_of(ConnectionId(2)).unwrap();
    state.outbox.clear();
    (state, alice, bob)
}

fn holding(state: &mut TickState, entity: EntityId, item: &str) {
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse(item).unwrap())
        .unwrap();
    let inventory = &mut state.players.get_mut(&entity).unwrap().inventory;
    inventory.selected = 4;
    inventory.slots[4] = ItemStack::new(item, 1);
}

fn invoke(state: &mut TickState, executor: EntityId, input: &str) -> CommandResult {
    execute(
        state,
        ConnectionId(1),
        executor,
        &parse(input, true).unwrap(),
        &mut ChangeSet::default(),
    )
}

fn success(target: &str, multiple: bool) -> PlayerChange {
    PlayerChange::StyledChat {
        content: translated(
            if multiple {
                "commands.enchant.success.multiple"
            } else {
                "commands.enchant.success.single"
            },
            vec![
                translated("enchantment.minecraft.sharpness", Vec::new()),
                text(target),
            ],
        ),
    }
}

#[test]
fn single_target_publishes_selected_slot_to_target_and_feedback_to_executor() {
    let (mut state, alice, bob) = fixture();
    holding(&mut state, bob, "diamond_sword");
    let inventory = &mut state.players.get_mut(&bob).unwrap().inventory;
    inventory.slots[4].custom_name = Some("Named sword".into());
    let mut expected = inventory.clone();
    expected.slots[4] = expected.slots[4]
        .clone()
        .with_enchantment(Identifier::parse("sharpness").unwrap(), 2);
    let before_revision = state.players[&bob].inventory_menu.state_id;

    assert_eq!(
        invoke(&mut state, alice, "enchant Bob sharpness 2"),
        CommandResult::Success(1)
    );
    assert_eq!(state.players[&bob].inventory, expected);
    assert_eq!(
        state.outbox[&ConnectionId(2)],
        vec![PlayerChange::MenuSlot {
            window: 0,
            state_id: (before_revision + 1) & 0x7FFF,
            slot: 40,
            item: Some(expected.slots[4].clone()),
        }]
    );
    assert_eq!(state.outbox[&ConnectionId(1)], vec![success("Bob", false)]);
}

#[test]
fn multi_target_feedback_counts_only_successful_targets() {
    let (mut state, alice, bob) = fixture();
    holding(&mut state, alice, "diamond_sword");
    holding(&mut state, bob, "stone");
    assert_eq!(
        invoke(&mut state, alice, "enchant @a sharpness 2"),
        CommandResult::Success(1)
    );
    assert_eq!(
        state.outbox[&ConnectionId(1)].last(),
        Some(&success("Alice", false))
    );
    assert!(!state.outbox.contains_key(&ConnectionId(2)));
    assert!(
        state.players[&bob].inventory.slots[4]
            .enchantments
            .is_empty()
    );

    // Alice now fails compatibility, Bob succeeds; failed targets are silent.
    state.outbox.clear();
    holding(&mut state, bob, "diamond_sword");
    assert_eq!(
        invoke(&mut state, alice, "enchant @a sharpness 2"),
        CommandResult::Success(1)
    );
    assert_eq!(state.outbox[&ConnectionId(1)], vec![success("Bob", false)]);
    assert!(matches!(
        state.outbox[&ConnectionId(2)][..],
        [PlayerChange::MenuSlot { .. }]
    ));

    state.outbox.clear();
    let before = [
        state.players[&alice].inventory.clone(),
        state.players[&bob].inventory.clone(),
    ];
    assert_eq!(
        invoke(&mut state, alice, "enchant @a sharpness 2"),
        CommandResult::Failed
    );
    assert_eq!(
        state.outbox[&ConnectionId(1)],
        vec![PlayerChange::StyledChat {
            content: translated("commands.enchant.failed", Vec::new()),
        }]
    );
    assert_eq!(state.players[&alice].inventory, before[0]);
    assert_eq!(state.players[&bob].inventory, before[1]);

    state.outbox.clear();
    holding(&mut state, alice, "diamond_sword");
    holding(&mut state, bob, "diamond_sword");
    assert_eq!(
        invoke(&mut state, alice, "enchant @a sharpness 2"),
        CommandResult::Success(2)
    );
    assert_eq!(
        state.outbox[&ConnectionId(1)].last(),
        Some(&success("2", true))
    );
}

#[test]
fn single_target_failures_keep_current_feedback_and_result_contracts() {
    let (mut state, alice, bob) = fixture();
    let before = state.players[&bob].inventory.clone();
    for (input, result, feedback) in [
        (
            "enchant Bob sharpness 2",
            CommandResult::Failed,
            PlayerChange::Chat {
                message: "Target is not holding an item".into(),
            },
        ),
        (
            "enchant Bob sharpness 6",
            CommandResult::Failed,
            PlayerChange::Chat {
                message: "Level 6 is too high for minecraft:sharpness (maximum 5)".into(),
            },
        ),
        (
            "enchant @a[name=Missing] sharpness 2",
            CommandResult::Failed,
            PlayerChange::StyledChat {
                content: translated("argument.entity.notfound.entity", Vec::new()),
            },
        ),
    ] {
        state.outbox.clear();
        assert_eq!(invoke(&mut state, alice, input), result, "{input}");
        assert_eq!(state.outbox[&ConnectionId(1)], vec![feedback], "{input}");
        assert_eq!(state.players[&bob].inventory, before, "{input}");
        assert!(!state.outbox.contains_key(&ConnectionId(2)));
    }
    state.spawn_mob(state.registry.cow_type_id, [0.0, 80.0, 0.0], 10.0);
    state.outbox.clear();
    assert_eq!(
        invoke(
            &mut state,
            alice,
            "enchant @e[type=minecraft:cow] sharpness 2"
        ),
        CommandResult::Failed
    );
    assert_eq!(
        state.outbox[&ConnectionId(1)],
        vec![PlayerChange::StyledChat {
            content: translated(
                "commands.enchant.failed.itemless",
                vec![translated("entity.minecraft.cow", Vec::new())],
            ),
        }]
    );
}

#[test]
fn zero_level_keeps_inventory_and_revision_but_still_reports_success() {
    let (mut state, alice, _) = fixture();
    holding(&mut state, alice, "diamond_sword");
    let before = state.players[&alice].inventory.clone();
    let revision = state.players[&alice].inventory_menu.state_id;
    assert_eq!(
        invoke(&mut state, alice, "enchant @s sharpness 0"),
        CommandResult::Success(1)
    );
    assert_eq!(state.players[&alice].inventory, before);
    assert_eq!(state.players[&alice].inventory_menu.state_id, revision);
    assert_eq!(
        state.outbox[&ConnectionId(1)],
        vec![success("Alice", false)]
    );
}

#[test]
fn execute_store_rejects_enchant_before_inventory_mutation() {
    let (mut state, alice, _) = fixture();
    holding(&mut state, alice, "diamond_sword");
    let context = crate::command_context::Context::source(&state, ConnectionId(1), alice, 2);
    crate::command_context::run(
        &mut state,
        "scoreboard objectives add oracle dummy",
        context,
        &mut ChangeSet::default(),
    );
    state.outbox.clear();
    let before = state.players[&alice].inventory.clone();
    crate::command_context::run(
        &mut state,
        "execute store result score rejected oracle run enchant @s sharpness 2",
        context,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.players[&alice].inventory, before);
    assert_eq!(state.scoreboard.scores["rejected"]["oracle"].value, 0);
    assert_eq!(
        state.outbox[&ConnectionId(1)][0],
        PlayerChange::Chat {
            message: "Command does not support execute store".into(),
        }
    );
    assert!(
        state.outbox[&ConnectionId(1)]
            .iter()
            .all(|change| !matches!(change, PlayerChange::MenuSlot { .. }))
    );
    assert!(state.command_context.current.is_none());
}
