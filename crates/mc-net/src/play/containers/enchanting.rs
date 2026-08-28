use mc_data::Identifier;
use mc_data::ItemStack;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_nbt::Tag;
use mc_world::BlockPos;

use crate::play::inventory::PlayerInventory;
use crate::play::persistence::XpState;
use crate::play::splitmix64;

pub(in crate::play) const ENCHANTING_MENU_TYPE_ID: i32 = 13;
pub(in crate::play) const ENCHANTING_MENU_SLOT_COUNT: usize = 38;

#[derive(Debug, Clone)]
pub(in crate::play) struct EnchantingTableWindow {
    pub(in crate::play) container_id: i32,
    pub(in crate::play) state_id: i32,
    pub(in crate::play) position: BlockPos,
    pub(in crate::play) inputs: [ItemStack; 2],
}

impl EnchantingTableWindow {
    pub(in crate::play) fn at_position(container_id: i32, position: BlockPos) -> Self {
        Self {
            container_id,
            state_id: 1,
            position,
            inputs: std::array::from_fn(|_| ItemStack::EMPTY),
        }
    }
}

pub(in crate::play) type EnchantingOffer = mc_data::enchanting_26_1_2::EnchantingOffer;

pub(in crate::play) fn enchanting_menu_title_nbt() -> Result<Vec<u8>, mc_protocol::CodecError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![(
            "text".to_string(),
            Tag::String("Enchanting".to_string()),
        )]),
    )?;
    Ok(out)
}

pub(in crate::play) fn enchanting_offer(
    bookshelf_count: u8,
    button_id: i32,
) -> Option<EnchantingOffer> {
    mc_data::enchanting_26_1_2::enchanting_offer(bookshelf_count, button_id)
}

pub(in crate::play) fn supported_enchantment_for_item(
    item_facts: &ItemFactsTable,
    item: &Identifier,
) -> Option<Identifier> {
    mc_data::enchanting_26_1_2::supported_enchantment_for_item(item_facts, item)
}

fn additional_enchantment_for_offer(
    item: &Identifier,
    offer: EnchantingOffer,
) -> Option<(Identifier, i32)> {
    mc_data::enchanting_26_1_2::additional_enchantment_for_offer(item, offer.button_id)
}

pub(in crate::play) fn enchant_item_candidate(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    inputs: &mut [ItemStack; 2],
    xp: &mut XpState,
    offer: EnchantingOffer,
) -> bool {
    if inputs[0].is_empty() || !inputs[0].enchantments.is_empty() {
        return false;
    }
    let Some(item) = items.name_of(inputs[0].item_id) else {
        return false;
    };
    let Some(enchantment) = supported_enchantment_for_item(item_facts, item) else {
        return false;
    };
    let additional_enchantment = additional_enchantment_for_offer(item, offer);

    let lapis = Identifier::parse("minecraft:lapis_lazuli").expect("static identifier");
    let Some(lapis_id) = items.id_of(&lapis) else {
        return false;
    };
    if inputs[1].is_empty()
        || inputs[1].item_id != lapis_id
        || inputs[1].count < offer.lapis_cost
        || xp.level < offer.required_level
    {
        return false;
    }

    let next_seed = splitmix64(
        xp.seed as u32 as u64
            ^ (u64::from(inputs[0].item_id) << 32)
            ^ xp.total as u32 as u64
            ^ offer.enchantment_level as u32 as u64,
    ) as u32 as i32;
    if !xp.spend_enchantment_levels(offer.lapis_cost, next_seed) {
        return false;
    }
    let mut enchanted = inputs[0]
        .clone()
        .with_enchantment(enchantment, offer.enchantment_level);
    if let Some((enchantment, level)) = additional_enchantment {
        enchanted = enchanted.with_enchantment(enchantment, level);
    }
    inputs[0] = enchanted;
    inputs[1].count -= offer.lapis_cost;
    if inputs[1].count <= 0 {
        inputs[1] = ItemStack::EMPTY;
    }
    true
}

pub(in crate::play) fn enchanting_data_values(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    window: &EnchantingTableWindow,
    xp: &XpState,
    bookshelf_count: u8,
) -> [(i16, i16); 10] {
    let item = &window.inputs[0];
    let enchantment = (!item.is_empty() && item.enchantments.is_empty())
        .then(|| items.name_of(item.item_id))
        .flatten()
        .and_then(|item| supported_enchantment_for_item(item_facts, item));
    let clue = enchantment
        .as_ref()
        .and_then(|enchantment| {
            mc_data::required_registry_entry_id("minecraft:enchantment", enchantment)
        })
        .and_then(|id| i16::try_from(id).ok())
        .unwrap_or(-1);
    let offers = std::array::from_fn::<_, 3, _>(|slot| {
        enchantment
            .as_ref()
            .and_then(|_| enchanting_offer(bookshelf_count, slot as i32))
    });
    let cost = |slot: usize| {
        offers[slot]
            .map(|offer| offer.required_level as i16)
            .unwrap_or(0)
    };
    let offer_clue = |slot: usize| offers[slot].map_or(-1, |_| clue);
    let offer_level = |slot: usize| {
        offers[slot]
            .map(|offer| offer.enchantment_level as i16)
            .unwrap_or(-1)
    };
    [
        (0, cost(0)),
        (1, cost(1)),
        (2, cost(2)),
        (3, xp.seed as i16),
        (4, offer_clue(0)),
        (5, offer_clue(1)),
        (6, offer_clue(2)),
        (7, offer_level(0)),
        (8, offer_level(1)),
        (9, offer_level(2)),
    ]
}

pub(in crate::play) fn count_valid_enchanting_bookshelves(
    table: BlockPos,
    mut is_provider: impl FnMut(BlockPos) -> bool,
    mut is_transmitter: impl FnMut(BlockPos) -> bool,
) -> u8 {
    let mut count = 0_u8;
    for y in 0..=1 {
        for x in -2_i32..=2 {
            for z in -2_i32..=2 {
                if x.abs() != 2 && z.abs() != 2 {
                    continue;
                }
                let provider = BlockPos {
                    x: table.x + x,
                    y: table.y + y,
                    z: table.z + z,
                };
                let transmitter = BlockPos {
                    x: table.x + x / 2,
                    y: table.y + y,
                    z: table.z + z / 2,
                };
                if is_provider(provider) && is_transmitter(transmitter) {
                    count = count.saturating_add(1).min(15);
                }
            }
        }
    }
    count
}

#[cfg(test)]
pub(in crate::play) fn item_is_efficiency_enchantable(
    item_facts: &ItemFactsTable,
    item: &Identifier,
) -> bool {
    mc_data::enchanting_26_1_2::item_is_efficiency_enchantable(item_facts, item)
}

pub(in crate::play) fn enchanting_table_input_projection(
    input: &[ItemStack; 2],
) -> Option<Box<[ItemStack; 2]>> {
    input
        .iter()
        .any(|stack| !stack.is_empty())
        .then(|| Box::new(input.clone()))
}

pub(in crate::play) fn enchanting_table_input_from_projection(
    input: Option<Box<[ItemStack; 2]>>,
) -> [ItemStack; 2] {
    input
        .map(|input| *input)
        .unwrap_or_else(|| std::array::from_fn(|_| ItemStack::EMPTY))
}

pub(in crate::play) fn enchanting_player_slot(menu_slot: usize) -> Option<usize> {
    match menu_slot {
        2..=28 => Some(9 + (menu_slot - 2)),
        29..=37 => Some(36 + (menu_slot - 29)),
        _ => None,
    }
}

pub(in crate::play) fn enchanting_wire_items(
    window: &EnchantingTableWindow,
    inventory: &PlayerInventory,
) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(ENCHANTING_MENU_SLOT_COUNT);
    items.extend(window.inputs.iter().cloned());
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

pub(in crate::play) fn enchanting_menu_stack(
    window: &EnchantingTableWindow,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    match menu_slot {
        0..=1 => Some(window.inputs[menu_slot].clone()),
        _ => enchanting_player_slot(menu_slot).map(|slot| inventory.slots[slot].clone()),
    }
}

pub(in crate::play) fn set_enchanting_menu_stack(
    window: &mut EnchantingTableWindow,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    match menu_slot {
        0..=1 => {
            window.inputs[menu_slot] = stack;
            true
        }
        _ => {
            let Some(slot) = enchanting_player_slot(menu_slot) else {
                return false;
            };
            inventory.slots[slot] = stack;
            true
        }
    }
}

pub(in crate::play) fn is_lapis_stack(items: &ItemRegistry, stack: &ItemStack) -> bool {
    !stack.is_empty()
        && items
            .name_of(stack.item_id)
            .is_some_and(|id| id.as_str() == "minecraft:lapis_lazuli")
}

pub(in crate::play) fn can_place_in_enchanting_menu_slot(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    menu_slot: usize,
    stack: &ItemStack,
    mut can_place_in_player_slot: impl FnMut(usize, &ItemStack) -> bool,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    match menu_slot {
        0 => items
            .name_of(stack.item_id)
            .and_then(|id| supported_enchantment_for_item(item_facts, id))
            .is_some(),
        1 => is_lapis_stack(items, stack),
        _ => enchanting_player_slot(menu_slot)
            .is_some_and(|slot| can_place_in_player_slot(slot, stack)),
    }
}
