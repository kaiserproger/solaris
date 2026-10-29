use mc_data::food::FoodEntry;
use mc_data::item_components::{ItemFacts, ItemFactsTable, ToolFacts};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::tags::TagsData;
use mc_data::{Identifier, ItemStack};

use super::{PlayerEffects, PlayerState};
use crate::boundary::{ClickMode, ConnectionId, Hand, PlayerChange};
use crate::commands::GameMode;
use crate::inventory::{ItemRules, MenuKind, OFF_HAND};

const STONE: u32 = 1;
const STEW: u32 = 2;
const BOWL: u32 = 3;
const APPLE: u32 = 4;

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}

#[test]
fn teleport_sequence_survives_respawn_wraps_like_vanilla_and_resets_on_reconnect() {
    let connection = ConnectionId(1);
    let mut player = PlayerState::new(connection, GameMode::Survival);
    assert_eq!(player.begin_teleport(), 1);
    assert_eq!(player.begin_teleport(), 2);
    let effects = player.respawn([0.5, 64.0, 0.5], 0.0, 0.0, Default::default());
    assert!(matches!(
        effects.changes[0],
        PlayerChange::Respawned { teleport_id: 3, .. }
    ));
    assert_eq!(player.awaiting_teleport, Some(3));
    player.teleport_id = i32::MAX - 2;
    assert_eq!(player.begin_teleport(), i32::MAX - 1);
    assert_eq!(player.begin_teleport(), 0);
    assert_eq!(player.begin_teleport(), 1);
    let mut reconnected = PlayerState::new(connection, GameMode::Survival);
    assert_eq!(reconnected.awaiting_teleport, None);
    assert_eq!(reconnected.begin_teleport(), 1);
}
#[test]
fn selecting_wooden_sword_and_empty_slot_diffs_attributes() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let tags = TagsData::default();
    let rules = ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    player.inventory.slots[1] =
        ItemStack::new(items.id_of(&id("minecraft:wooden_sword")).unwrap(), 1);
    player.equipment_attributes(&rules, &[]).unwrap();
    player.inventory.selected = 1;
    let Some(PlayerChange::Attributes { attributes, .. }) =
        player.equipment_attributes(&rules, &[])
    else {
        panic!("selection must publish changed attributes");
    };
    let speed = attributes
        .iter()
        .find(|a| a.kind == "attack_speed")
        .unwrap();
    // Items.WOODEN_SWORD passes -2.4f to Properties.sword; vanilla promotes
    // that float to the serialized double, rather than rounding it to -2.4.
    assert_eq!(speed.modifiers[0].amount, f64::from(-2.4_f32));
    assert_eq!(
        speed.base + speed.modifiers[0].amount,
        4.0 + f64::from(-2.4_f32)
    );
    assert!(player.equipment_attributes(&rules, &[]).is_none());
    player.inventory.selected = 0;
    let Some(PlayerChange::Attributes { attributes, .. }) =
        player.equipment_attributes(&rules, &[])
    else {
        panic!("empty hand must restore base");
    };
    let speed = attributes
        .iter()
        .find(|a| a.kind == "attack_speed")
        .unwrap();
    assert_eq!(speed.base, 4.0);
    assert!(speed.modifiers.is_empty());
}

struct Data {
    items: ItemRegistry,
    facts: ItemFactsTable,
    tags: TagsData,
}

impl Data {
    fn new() -> Self {
        let report = |name, protocol_id| ItemReport {
            id: id(name),
            protocol_id,
        };
        Self {
            items: ItemRegistry::from_report(&[
                report("minecraft:stone", STONE),
                report("minecraft:mushroom_stew", STEW),
                report("minecraft:bowl", BOWL),
                report("minecraft:apple", APPLE),
            ]),
            facts: ItemFactsTable::from_entries([
                (
                    id("minecraft:mushroom_stew"),
                    ItemFacts {
                        max_stack_size: Some(1),
                        food: Some(FoodEntry {
                            food: 6,
                            saturation: 7.2,
                            can_always_eat: false,
                        }),
                        use_duration_ticks: Some(32),
                        use_remainder: Some(id("minecraft:bowl")),
                        ..ItemFacts::default()
                    },
                ),
                (
                    id("minecraft:apple"),
                    ItemFacts {
                        food: Some(FoodEntry {
                            food: 4,
                            saturation: 2.4,
                            can_always_eat: false,
                        }),
                        ..ItemFacts::default()
                    },
                ),
            ]),
            tags: TagsData::default(),
        }
    }

    fn rules(&self) -> ItemRules<'_> {
        ItemRules {
            items: &self.items,
            facts: &self.facts,
            tags: &self.tags,
            recipes: &[],
        }
    }
}

fn player() -> PlayerState {
    PlayerState::new(ConnectionId(1), GameMode::Survival)
}

/// Total count of `item` held anywhere by the player plus `dropped`.
fn total(player: &PlayerState, effects: &PlayerEffects, item: u32) -> i32 {
    let menus = std::iter::once(&player.inventory_menu).chain(&player.open);
    player
        .inventory
        .slots
        .iter()
        .chain(menus.flat_map(|menu| menu.grid.iter().chain([&menu.carried])))
        .chain(&effects.dropped)
        .filter(|stack| stack.item_id == item && !stack.is_empty())
        .map(|stack| stack.count)
        .sum()
}

#[test]
fn eating_finishes_after_the_use_time_and_leaves_the_bowl() {
    let data = Data::new();
    let rules = data.rules();
    let mut p = player();
    p.vitals.food = 10;
    p.inventory.slots[0] = ItemStack::new(STEW, 1);
    p.use_item(&rules, Hand::Main);
    for _ in 0..31 {
        assert_eq!(p.tick_item_use(&rules), PlayerEffects::default());
    }
    assert_eq!(p.vitals.food, 10);
    let done = p.tick_item_use(&rules);
    assert_eq!(p.vitals.food, 16);
    assert_eq!(p.inventory.slots[0], ItemStack::new(BOWL, 1));
    assert!(done.changes.contains(&PlayerChange::MenuSlot {
        window: 0,
        state_id: 1,
        slot: 36,
        item: Some(ItemStack::new(BOWL, 1)),
    }));
    assert!(done.dropped.is_empty());
}

#[test]
fn releasing_or_switching_slot_before_the_end_eats_nothing() {
    let data = Data::new();
    let rules = data.rules();
    let mut p = player();
    p.vitals.food = 10;
    p.inventory.slots[0] = ItemStack::new(APPLE, 5);
    p.use_item(&rules, Hand::Main);
    for _ in 0..10 {
        p.tick_item_use(&rules);
    }
    p.release_use_item();
    p.use_item(&rules, Hand::Main);
    p.select_slot(1);
    for _ in 0..40 {
        p.tick_item_use(&rules);
    }
    assert_eq!(p.vitals.food, 10);
    assert_eq!(p.inventory.slots[0], ItemStack::new(APPLE, 5));
}

#[test]
fn closing_the_inventory_returns_grid_and_cursor_without_loss() {
    let data = Data::new();
    let rules = data.rules();
    let mut p = player();
    p.inventory_menu.grid[0] = ItemStack::new(STONE, 3);
    p.inventory_menu.carried = ItemStack::new(STONE, 2);
    let closed = p.close(&rules);
    assert!(closed.dropped.is_empty());
    assert_eq!(p.inventory.slots[0], ItemStack::new(STONE, 5));
    assert_eq!(total(&p, &closed, STONE), 5);
    assert!(closed.changes.iter().any(|change| matches!(
        change,
        PlayerChange::MenuSlot { window: 0, slot: 36, item: Some(stack), .. }
            if *stack == ItemStack::new(STONE, 5)
    )));
}

#[test]
fn integrity_open_menu_refunds_window_zero_custody_once() {
    let data = Data::new();
    let rules = data.rules();
    for full in [false, true] {
        for cursor in [false, true] {
            let mut p = player();
            if full {
                p.inventory.slots.fill(ItemStack::new(APPLE, 64));
            }
            let stack = ItemStack::new(STONE, 3);
            if cursor {
                p.inventory_menu.carried = stack;
            } else {
                p.inventory_menu.grid[0] = stack;
            }
            let effects = p.open_menu(
                &rules,
                MenuKind::Chest { rows: 3 },
                "Chest".into(),
                &vec![ItemStack::EMPTY; 27],
            );
            assert_eq!(total(&p, &effects, STONE), 3);
            assert!(p.inventory_menu.carried.is_empty());
            assert!(p.inventory_menu.grid.iter().all(ItemStack::is_empty));
            assert_eq!(
                effects.dropped.iter().map(|stack| stack.count).sum::<i32>(),
                if full { 3 } else { 0 }
            );
            let closed = p.close(&rules);
            assert!(closed.dropped.is_empty());
        }
    }
}

#[test]
fn death_drops_every_stack_exactly_once() {
    let data = Data::new();
    let rules = data.rules();
    let mut p = player();
    p.inventory.slots[5] = ItemStack::new(STONE, 10);
    p.inventory.slots[OFF_HAND] = ItemStack::new(APPLE, 3);
    p.vitals.experience.add_points(42);
    p.open_menu(&rules, MenuKind::CraftingTable, "Crafting".into(), &[]);
    let menu = p.open.as_mut().unwrap();
    menu.grid[4] = ItemStack::new(STONE, 2);
    menu.carried = ItemStack::new(APPLE, 1);
    let died = p.die(&rules, false, false);
    assert!(p.open.is_none());
    assert!(p.inventory.slots.iter().all(ItemStack::is_empty));
    assert_eq!(total(&p, &died, STONE), 12);
    assert_eq!(total(&p, &died, APPLE), 4);
    assert_eq!(p.vitals.experience, crate::survival::Experience::default());
}

#[test]
fn keep_inventory_preserves_carried_stacks_and_experience_through_respawn() {
    let data = Data::new();
    let rules = data.rules();
    let mut p = player();
    p.inventory.slots[5] = ItemStack::new(STONE, 10);
    p.vitals.experience.add_points(42);
    let experience = p.vitals.experience;
    p.open_menu(&rules, MenuKind::CraftingTable, "Crafting".into(), &[]);
    p.open.as_mut().unwrap().carried = ItemStack::new(APPLE, 1);
    let died = p.die(&rules, true, false);
    assert!(p.open.is_none());
    assert!(died.dropped.is_empty());
    assert_eq!(p.inventory.slots[5], ItemStack::new(STONE, 10));
    assert_eq!(p.vitals.experience, experience);
    p.respawn([0.5, 64.0, 0.5], 0.0, 0.0, Default::default());
    assert_eq!(p.vitals.experience, experience);
    assert_eq!(p.vitals.health, 20.0);
}

#[test]
fn vanishing_curse_is_destroyed_only_when_inventory_is_not_kept() {
    let data = Data::new();
    let rules = data.rules();
    for keep_inventory in [false, true] {
        let mut p = player();
        let mut stack = ItemStack::new(STONE, 1);
        stack.enchantments.push(mc_data::ItemEnchantment {
            id: id("minecraft:vanishing_curse"),
            level: 1,
        });
        p.inventory.slots[0] = stack.clone();
        let effects = p.die(&rules, keep_inventory, false);
        assert!(effects.dropped.is_empty());
        assert_eq!(
            p.inventory.slots[0],
            if keep_inventory {
                stack
            } else {
                ItemStack::EMPTY
            }
        );
    }
}

#[test]
fn clicks_on_a_window_that_is_not_current_are_ignored() {
    let data = Data::new();
    let rules = data.rules();
    let mut p = player();
    p.inventory.slots[9] = ItemStack::new(STONE, 4);
    let opened = p.open_menu(&rules, MenuKind::CraftingTable, "Crafting".into(), &[]);
    assert!(matches!(
        opened.changes[0],
        PlayerChange::MenuOpened { window: 1, .. }
    ));
    let before = p.clone();
    let stale = p.click(&rules, 0, 0, 9, 0, ClickMode::Pickup, &mut []);
    assert_eq!(stale, PlayerEffects::default());
    assert_eq!(p, before);
}

#[test]
fn swapping_hands_updates_the_selected_hotbar_and_off_hand_menu_slots() {
    let mut p = player();
    p.inventory.selected = 2;
    p.inventory.slots[2] = ItemStack::new(STONE, 1);
    let swapped = p.swap_hands();
    assert_eq!(p.inventory.slots[OFF_HAND], ItemStack::new(STONE, 1));
    let slots: Vec<(i16, Option<ItemStack>)> = swapped
        .changes
        .into_iter()
        .filter_map(|change| match change {
            PlayerChange::MenuSlot { slot, item, .. } => Some((slot, item)),
            _ => None,
        })
        .collect();
    assert_eq!(
        slots,
        vec![(38, None), (45, Some(ItemStack::new(STONE, 1)))]
    );
}

/// Tool wear accumulates damage and breaks the tool at max damage.
#[test]
fn tool_wear_accumulates_and_breaks_at_max_damage() {
    struct Data {
        items: ItemRegistry,
        facts: ItemFactsTable,
        tags: TagsData,
    }
    let data = Data {
        items: ItemRegistry::from_report(&[ItemReport {
            id: Identifier::parse("minecraft:iron_pickaxe").unwrap(),
            protocol_id: 9,
        }]),
        facts: ItemFactsTable::from_entries([(
            Identifier::parse("minecraft:iron_pickaxe").unwrap(),
            ItemFacts {
                max_damage: Some(3),
                tool: Some(ToolFacts {
                    default_mining_speed: None,
                    damage_per_block: Some(1),
                    rules: Vec::new(),
                }),
                ..ItemFacts::default()
            },
        )]),
        tags: TagsData::default(),
    };
    let rules = ItemRules {
        items: &data.items,
        facts: &data.facts,
        tags: &data.tags,
        recipes: &[],
    };
    let mut p = player();
    p.inventory.slots[p.inventory.selected as usize] = ItemStack::new(9, 1);
    let worn = p.damage_held_tool(&rules, 2);
    assert_eq!(
        p.inventory.slots[p.inventory.selected as usize].damage,
        Some(2)
    );
    assert!(!worn.changes.is_empty(), "wear syncs the slot");
    let broken = p.damage_held_tool(&rules, 1);
    assert!(
        p.inventory.slots[p.inventory.selected as usize].is_empty(),
        "tool breaks"
    );
    assert!(!broken.changes.is_empty(), "break syncs the slot");
}

#[test]
fn a_click_syncs_slots_and_cursor_as_one_image() {
    // Separate slot and cursor packets let a client see the stack in both
    // places (or neither) between them; one image per click cannot.
    let data = Data::new();
    let rules = data.rules();
    let mut p = player();
    p.inventory.slots[9] = ItemStack::new(APPLE, 2);
    let picked = p.click(&rules, 0, 0, 9, 0, ClickMode::Pickup, &mut []);
    let [PlayerChange::MenuContents { slots, cursor, .. }] = picked.changes.as_slice() else {
        panic!("one MenuContents per click: {:?}", picked.changes);
    };
    assert_eq!(*cursor, Some(ItemStack::new(APPLE, 2)));
    assert_eq!(slots[9], None);
    let placed = p.click(
        &rules,
        0,
        p.inventory_menu.state_id,
        9,
        0,
        ClickMode::Pickup,
        &mut [],
    );
    let [PlayerChange::MenuContents { slots, cursor, .. }] = placed.changes.as_slice() else {
        panic!("one MenuContents per click: {:?}", placed.changes);
    };
    assert_eq!(*cursor, None);
    assert_eq!(slots[9], Some(ItemStack::new(APPLE, 2)));
}

#[test]
fn creative_slot_sets_menu_slots_and_throws_only_for_creative_players() {
    let data = Data::new();
    let rules = data.rules();
    let mut survival = player();
    let refused = survival.creative_slot(&rules, 36, ItemStack::new(STONE, 64));
    assert!(survival.inventory.slots[0].is_empty());
    assert!(refused.changes.is_empty() && refused.dropped.is_empty());

    let mut p = PlayerState::new(ConnectionId(1), GameMode::Creative);
    p.creative_slot(&rules, 36, ItemStack::new(STONE, 64));
    p.creative_slot(&rules, 5, ItemStack::new(APPLE, 1));
    p.creative_slot(&rules, 45, ItemStack::new(APPLE, 2));
    assert_eq!(p.inventory.slots[0], ItemStack::new(STONE, 64));
    assert_eq!(p.inventory.slots[39], ItemStack::new(APPLE, 1));
    assert_eq!(p.inventory.slots[40], ItemStack::new(APPLE, 2));

    let overstacked = p.creative_slot(&rules, 9, ItemStack::new(STEW, 2));
    assert!(p.inventory.slots[9].is_empty() && overstacked.changes.is_empty());

    let thrown = p.creative_slot(&rules, -1, ItemStack::new(STONE, 3));
    assert_eq!(thrown.dropped, vec![ItemStack::new(STONE, 3)]);
}

#[test]
fn creative_slot_rejects_unknown_native_ids_before_inventory_or_drop_mutation() {
    let data = Data::new();
    let rules = data.rules();
    let mut p = PlayerState::new(ConnectionId(1), GameMode::Creative);
    p.inventory.slots[0] = ItemStack::new(STONE, 3);
    let before = p.inventory.slots.clone();
    for slot in [36, -1] {
        let effects = p.creative_slot(&rules, slot, ItemStack::new(u32::MAX, 1));
        assert!(effects.changes.is_empty() && effects.dropped.is_empty());
        assert_eq!(p.inventory.slots, before);
    }
}

#[test]
fn rejected_current_window_click_restores_slots_and_cursor_in_one_authoritative_image() {
    let data = Data::new();
    let rules = data.rules();
    let mut p = player();
    p.inventory.slots[9] = ItemStack::new(APPLE, 2);
    p.inventory_menu.carried = ItemStack::new(STONE, 1);
    let state_id = p.inventory_menu.state_id;
    // The client predicts that stone can enter the helmet slot; the server
    // refuses. Predictions are not part of the authoritative click input.
    let effects = p.click(&rules, 0, state_id, 5, 0, ClickMode::Pickup, &mut []);
    let [
        PlayerChange::MenuContents {
            window,
            state_id: published,
            slots,
            cursor,
            ..
        },
    ] = effects.changes.as_slice()
    else {
        panic!("a refusal must publish exactly one full authoritative image");
    };
    assert_eq!((*window, *published), (0, state_id));
    assert!(slots[5].is_none());
    assert_eq!(slots[9], Some(ItemStack::new(APPLE, 2)));
    assert_eq!(*cursor, Some(ItemStack::new(STONE, 1)));
    assert!(effects.dropped.is_empty());
    assert!(
        p.click(&rules, 1, state_id, 5, 0, ClickMode::Pickup, &mut [])
            .changes
            .is_empty()
    );
}

#[test]
fn use_equippable_swaps_armor_and_preserves_creative_hand() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let tags = TagsData::default();
    let rules = ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    for (new, old, target) in [
        ("diamond_helmet", "iron_helmet", 39),
        ("diamond_chestplate", "iron_chestplate", 38),
        ("elytra", "diamond_chestplate", 38),
    ] {
        for creative in [false, true] {
            let mut player = PlayerState::new(
                ConnectionId(1),
                if creative {
                    GameMode::Creative
                } else {
                    GameMode::Survival
                },
            );
            let held = ItemStack::new(items.id_of(&id(&format!("minecraft:{new}"))).unwrap(), 1);
            let old = ItemStack::new(items.id_of(&id(&format!("minecraft:{old}"))).unwrap(), 1);
            player.inventory.slots[0] = held.clone();
            player.inventory.slots[target] = old.clone();
            let effects = player.swap_equippable(&rules, Hand::Main, 155).unwrap();
            assert_eq!(player.inventory.slots[target], held);
            assert_eq!(player.inventory.slots[0], old);
            assert!(player.inventory.slots[1].is_empty());
            assert!(effects.dropped.is_empty());
            assert!(matches!(
                effects.changes.as_slice(),
                [PlayerChange::MenuContents { .. }]
            ));
        }
        let mut player = PlayerState::new(ConnectionId(1), GameMode::Creative);
        let held = ItemStack::new(items.id_of(&id(&format!("minecraft:{new}"))).unwrap(), 1);
        player.inventory.slots[0] = held.clone();
        player.swap_equippable(&rules, Hand::Main, 155).unwrap();
        assert_eq!(player.inventory.slots[0], held);
        assert_eq!(player.inventory.slots[target], held);
    }
}

#[test]
fn use_equippable_refuses_binding_and_identical_components() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let tags = TagsData::default();
    let rules = ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    let held = ItemStack::new(items.id_of(&id("minecraft:diamond_helmet")).unwrap(), 1);
    for creative in [false, true] {
        let mut player = PlayerState::new(
            ConnectionId(1),
            if creative {
                GameMode::Creative
            } else {
                GameMode::Survival
            },
        );
        player.inventory.slots[0] = held.clone();
        let mut cursed = ItemStack::new(items.id_of(&id("minecraft:iron_helmet")).unwrap(), 1);
        cursed.enchantments.push(mc_data::ItemEnchantment {
            id: id("minecraft:binding_curse"),
            level: 1,
        });
        player.inventory.slots[39] = cursed.clone();
        let effects = player.swap_equippable(&rules, Hand::Main, 155).unwrap();
        assert_eq!(
            player.inventory.slots[39],
            if creative { held.clone() } else { cursed }
        );
        assert_eq!(effects.changes.is_empty(), !creative);
    }
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    player.inventory.slots[0] = held.clone();
    player.inventory.slots[39] = held.clone();
    assert!(
        player
            .swap_equippable(&rules, Hand::Main, 155)
            .unwrap()
            .changes
            .is_empty()
    );
    player.inventory.slots[39].damage = Some(1);
    assert!(
        !player
            .swap_equippable(&rules, Hand::Main, 155)
            .unwrap()
            .changes
            .is_empty()
    );
    assert_eq!(player.inventory.slots[0].damage, Some(1));
}

#[test]
fn use_stacked_equippable_consumes_one_and_returns_or_drops_old() {
    let items = mc_data::items::solaris_required_items();
    let facts = ItemFactsTable::from_entries([(
        id("minecraft:stone"),
        ItemFacts {
            max_stack_size: Some(64),
            equippable_slot: Some("head".into()),
            ..ItemFacts::default()
        },
    )]);
    let tags = TagsData::default();
    let rules = ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    let held = ItemStack::new(items.id_of(&id("minecraft:stone")).unwrap(), 3);
    let old = ItemStack::new(items.id_of(&id("minecraft:iron_helmet")).unwrap(), 1);
    for full in [false, true] {
        let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
        if full {
            player.inventory.slots[..36].fill(ItemStack::new(
                items.id_of(&id("minecraft:dirt")).unwrap(),
                64,
            ));
        }
        player.inventory.slots[0] = held.clone();
        player.inventory.slots[39] = old.clone();
        let effects = player.swap_equippable(&rules, Hand::Main, 155).unwrap();
        assert_eq!(player.inventory.slots[0].count, 2);
        assert_eq!(player.inventory.slots[39].count, 1);
        assert_eq!(
            effects.dropped,
            if full { vec![old.clone()] } else { vec![] }
        );
        if !full {
            assert_eq!(player.inventory.slots[1], old);
        }
    }
}

#[test]
fn use_equippable_checks_swappable_slot_and_allowed_entities() {
    let items = mc_data::items::solaris_required_items();
    let tags = TagsData::default();
    for (slot, swappable, allowed, equips) in [
        ("head", true, None, true),
        ("head", false, None, false),
        ("body", true, None, false),
        ("head", true, Some(vec!["minecraft:cow".into()]), false),
        ("head", true, Some(vec!["minecraft:player".into()]), true),
    ] {
        let facts = ItemFactsTable::from_entries([(
            id("minecraft:stone"),
            ItemFacts {
                equippable_slot: Some(slot.into()),
                equippable_swappable: Some(swappable),
                equippable_allowed_entities: allowed,
                ..ItemFacts::default()
            },
        )]);
        let rules = ItemRules {
            items: &items,
            facts: &facts,
            tags: &tags,
            recipes: &[],
        };
        let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
        player.inventory.slots[OFF_HAND] =
            ItemStack::new(items.id_of(&id("minecraft:stone")).unwrap(), 1);
        assert_eq!(
            player.swap_equippable(&rules, Hand::Off, 155).is_some(),
            equips
        );
        assert_eq!(!player.inventory.slots[39].is_empty(), equips);
    }
}
