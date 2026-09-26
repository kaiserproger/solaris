use std::sync::Arc;

use super::{
    Identifier, ItemStack, PlayerInventory, attack_damage_for_item, fluid_test_registry,
    held_attack_damage, interaction_state_for_blocks,
};

#[test]
fn held_sharpness_uses_the_vanilla_26_1_2_damage_formula() {
    let items = Arc::new(mc_data::items::solaris_required_items());
    let sword = items
        .id_of(&Identifier::parse("minecraft:stone_sword").unwrap())
        .unwrap();
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    state.items = Arc::clone(&items);
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(sword, 1)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 3);

    assert_eq!(
        attack_damage_for_item(&state.item_facts, &state.items, Some(sword)),
        5.0
    );
    assert_eq!(
        held_attack_damage(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot()).unwrap(),
        ),
        7.0
    );
}

#[test]
fn registered_paper_carrier_uses_custom_weapon_damage_and_wear() {
    use mc_data::item_components::{CustomItemDefinition, ItemFacts};

    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    let model = Identifier::parse("ruby-live:blade").unwrap();
    state.items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:paper").unwrap(),
            protocol_id: 1,
        },
    ]));
    let paper = Identifier::parse("minecraft:paper").unwrap();
    let paper_id = state.items.id_of(&paper).unwrap();
    state.item_facts = Arc::new(
        mc_data::item_components::ItemFactsTable::default()
            .with_custom_items(
                [CustomItemDefinition {
                    id: model.clone(),
                    carrier: paper,
                    name: "Ruby Blade".to_owned(),
                    crafting_ingredient: None,
                    facts: ItemFacts {
                        max_stack_size: Some(1),
                        max_damage: Some(2),
                        weapon: true,
                        attack_damage_modifier: Some(5.0),
                        attack_speed_modifier: Some(-2.0),
                        ..ItemFacts::default()
                    },
                }],
                &state.items,
            )
            .unwrap(),
    );
    let mut blade = ItemStack::new(paper_id, 1).with_item_model(model);
    assert_eq!(
        held_attack_damage(&state.item_facts, &state.items, &blade),
        6.0
    );
    assert_eq!(
        super::held_attack_speed(&state.item_facts, &state.items, &blade),
        2.0
    );
    let vanilla_paper = ItemStack::new(paper_id, 1);
    assert_eq!(
        held_attack_damage(&state.item_facts, &state.items, &vanilla_paper),
        1.0
    );
    assert_eq!(
        super::damage_held_weapon_stack(&state.items, &state.item_facts, &mut blade)
            .unwrap()
            .damage,
        Some(1)
    );
    assert_eq!(
        super::damage_held_weapon_stack(&state.items, &state.item_facts, &mut blade),
        Some(ItemStack::EMPTY)
    );
}
#[tokio::test]
async fn held_weapon_tracks_owner_selection_and_rejected_selection_preserves_it() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    state.items = Arc::new(mc_data::items::solaris_required_items());
    let sword = state
        .items
        .id_of(&Identifier::parse("minecraft:stone_sword").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(sword, 1);
    state.inventory.slots[PlayerInventory::HOTBAR_BASE + 1] = ItemStack::new(sword, 1)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 3);
    let (stop, task) = super::start_survival_test_owner(
        &mut state,
        "OwnerSelectedWeapon",
        super::SurvivalState::FULL,
        &super::XpState::default(),
    );
    let damage = |state: &super::InteractionState| {
        held_attack_damage(
            &state.item_facts,
            &state.items,
            crate::play::survival::held_item_stack(state).unwrap(),
        )
    };

    state
        .simulation
        .commit_selected_hotbar_slot(1)
        .await
        .unwrap();
    let selected_damage = damage(&state);
    let invalid = state.simulation.commit_selected_hotbar_slot(9).await;
    let damage_after_rejection = damage(&state);
    state
        .simulation
        .commit_selected_hotbar_slot(0)
        .await
        .unwrap();
    let restored_damage = damage(&state);
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(selected_damage, 7.0);
    assert!(matches!(
        invalid,
        Err(crate::play::simulation::SimulationRequestError::InvalidCommand)
    ));
    assert_eq!(damage_after_rejection, 7.0);
    assert_eq!(restored_damage, 5.0);
}
