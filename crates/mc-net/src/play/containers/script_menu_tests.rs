use mc_data::items::solaris_required_items;
use mc_protocol::packets::play::{ContainerInput, ItemStack};
use mc_script::{
    ScriptInventoryClick, ScriptInventoryMenu, ScriptInventoryMenuItem, ScriptInventoryMenuSlot,
    ScriptPlayerId,
};

use super::script_menu::{
    ScriptMenuClick, ScriptMenuClickDisposition, ScriptMenuLayout, ScriptMenuOpenError,
    client_close_matches, close_identity_matches,
};
use crate::play::inventory::PlayerInventory;

fn menu(slots: &[(u8, &str, u8, Option<&str>)]) -> ScriptInventoryMenu {
    ScriptInventoryMenu::try_new(
        "catalog",
        "Catalog",
        slots
            .iter()
            .map(|(slot, resource, count, label)| {
                ScriptInventoryMenuSlot::new(
                    *slot,
                    ScriptInventoryMenuItem::try_new(resource, *count, label.map(str::to_owned))
                        .unwrap(),
                )
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn menu_rows_follow_the_highest_fixed_slot_and_labels_are_preserved() {
    let layout = ScriptMenuLayout::open(
        menu(&[(17, "minecraft:apple", 1, Some("Apple"))]),
        &solaris_required_items(),
    )
    .unwrap();

    assert_eq!(layout.rows(), 2);
    assert_eq!(layout.slots().len(), 18);
    assert_eq!(layout.slots()[17].custom_name.as_deref(), Some("Apple"));
}

#[test]
fn highest_supported_slot_produces_the_exact_six_row_layout() {
    let layout = ScriptMenuLayout::open(
        menu(&[(53, "minecraft:apple", 1, None)]),
        &solaris_required_items(),
    )
    .unwrap();

    assert_eq!(layout.rows(), 6);
    assert_eq!(layout.slots().len(), 54);
    assert_eq!(
        layout.wire_items(&PlayerInventory::empty()).len(),
        54 + 27 + 9
    );
}

#[test]
fn empty_menu_still_uses_one_generic_row() {
    let layout = ScriptMenuLayout::open(menu(&[]), &solaris_required_items()).unwrap();

    assert_eq!(layout.rows(), 1);
    assert_eq!(layout.slots().len(), 9);
    assert_eq!(layout.wire_items(&PlayerInventory::empty()).len(), 45);
}

#[test]
fn fixed_slot_primary_click_is_the_typed_menu_click() {
    let layout = ScriptMenuLayout::open(
        menu(&[(0, "minecraft:apple", 1, None)]),
        &solaris_required_items(),
    )
    .unwrap();

    let disposition = layout.classify_click(ScriptMenuClick::primary(4, 7, 0));

    assert_eq!(
        disposition,
        ScriptMenuClickDisposition::Clicked {
            slot: 0,
            click: ScriptInventoryClick::Primary,
        }
    );
}

#[test]
fn stale_and_player_inventory_clicks_resync_without_events() {
    let layout = ScriptMenuLayout::open(
        menu(&[(0, "minecraft:apple", 1, None)]),
        &solaris_required_items(),
    )
    .unwrap();

    assert_eq!(
        layout.classify_click(ScriptMenuClick::primary(5, 7, 0)),
        ScriptMenuClickDisposition::Resync
    );
    assert_eq!(
        layout.classify_click(ScriptMenuClick::primary(4, 6, 0)),
        ScriptMenuClickDisposition::Resync
    );
    assert_eq!(
        layout.classify_click(ScriptMenuClick::primary(4, 7, 9)),
        ScriptMenuClickDisposition::Resync
    );
}

#[test]
fn forged_and_unsupported_clicks_resync_without_changing_fixed_slots() {
    let layout = ScriptMenuLayout::open(
        menu(&[(0, "minecraft:apple", 1, None)]),
        &solaris_required_items(),
    )
    .unwrap();

    assert_eq!(
        layout.classify_click(ScriptMenuClick::from_packet(
            4,
            7,
            5,
            7,
            0,
            ContainerInput::Pickup,
            0
        )),
        ScriptMenuClickDisposition::Resync
    );
    assert_eq!(
        layout.classify_click(ScriptMenuClick::from_packet(
            4,
            7,
            4,
            7,
            0,
            ContainerInput::Swap,
            0
        )),
        ScriptMenuClickDisposition::Resync
    );
    for slot in [-999, -1] {
        assert_eq!(
            layout.classify_click(ScriptMenuClick::from_packet(
                4,
                7,
                4,
                7,
                slot,
                ContainerInput::Pickup,
                0,
            )),
            ScriptMenuClickDisposition::Resync
        );
    }
    assert_eq!(layout.slots()[0].count, 1);
}

#[test]
fn only_supported_buttons_and_modes_on_populated_fixed_slots_are_accepted() {
    let layout = ScriptMenuLayout::open(
        menu(&[(0, "minecraft:apple", 1, None)]),
        &solaris_required_items(),
    )
    .unwrap();

    for (input, button, expected) in [
        (ContainerInput::Pickup, 0, ScriptInventoryClick::Primary),
        (ContainerInput::Pickup, 1, ScriptInventoryClick::Secondary),
        (
            ContainerInput::QuickMove,
            0,
            ScriptInventoryClick::ShiftPrimary,
        ),
        (
            ContainerInput::QuickMove,
            1,
            ScriptInventoryClick::ShiftSecondary,
        ),
    ] {
        assert_eq!(
            layout.classify_click(ScriptMenuClick::from_packet(4, 7, 4, 7, 0, input, button,)),
            ScriptMenuClickDisposition::Clicked {
                slot: 0,
                click: expected,
            }
        );
    }

    for (input, button) in [
        (ContainerInput::Pickup, -1),
        (ContainerInput::Pickup, 2),
        (ContainerInput::QuickMove, 2),
        (ContainerInput::Swap, 0),
        (ContainerInput::Clone, 0),
        (ContainerInput::Throw, 0),
        (ContainerInput::QuickCraft, 0),
        (ContainerInput::PickupAll, 0),
    ] {
        assert_eq!(
            layout.classify_click(ScriptMenuClick::from_packet(4, 7, 4, 7, 0, input, button,)),
            ScriptMenuClickDisposition::Resync
        );
    }
    assert_eq!(
        layout.classify_click(ScriptMenuClick::from_packet(
            4,
            7,
            4,
            7,
            1,
            ContainerInput::Pickup,
            0,
        )),
        ScriptMenuClickDisposition::Resync
    );
}

#[test]
fn wire_items_append_only_main_inventory_and_hotbar() {
    let layout = ScriptMenuLayout::open(
        menu(&[(0, "minecraft:apple", 1, None)]),
        &solaris_required_items(),
    )
    .unwrap();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[8] = ItemStack::new(91, 1);
    inventory.slots[9] = ItemStack::new(92, 1);
    inventory.slots[44] = ItemStack::new(93, 1);

    let items = layout.wire_items(&inventory);

    assert_eq!(items.len(), 45);
    assert_eq!(items[0].count, 1);
    assert_eq!(items[9].item_id, 92);
    assert_eq!(items[44].item_id, 93);
    assert!(items.iter().all(|stack| stack.item_id != 91));
}

#[test]
fn unknown_items_are_rejected_before_opening() {
    assert!(matches!(
        ScriptMenuLayout::open(
            menu(&[(0, "minecraft:not_an_item", 1, None)]),
            &solaris_required_items(),
        ),
        Err(ScriptMenuOpenError::UnknownItem(item)) if item == "minecraft:not_an_item"
    ));
}

#[test]
fn close_requires_exact_plugin_player_and_menu_owner() {
    let owner = ScriptPlayerId::new(7);

    assert!(close_identity_matches(
        "catalog", owner, "main", "catalog", owner, "main",
    ));
    assert!(!close_identity_matches(
        "catalog", owner, "main", "forged", owner, "main",
    ));
    assert!(!close_identity_matches(
        "catalog",
        owner,
        "main",
        "catalog",
        ScriptPlayerId::new(8),
        "main",
    ));
    assert!(!close_identity_matches(
        "catalog", owner, "main", "catalog", owner, "other",
    ));
}

#[test]
fn client_close_requires_the_active_container_id() {
    assert!(client_close_matches(4, 4));
    assert!(!client_close_matches(4, 0));
    assert!(!client_close_matches(4, 3));
    assert!(!client_close_matches(4, 5));
}
