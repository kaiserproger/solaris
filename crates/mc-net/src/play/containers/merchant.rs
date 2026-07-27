use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_entity::villager_merchant_26_1_2::{VillagerMerchantState, VillagerTradeOffer};
use mc_nbt::Tag;
use mc_protocol::packets::play::{ItemStack, MerchantItemCost, MerchantOffer};

use crate::play::inventory::{PlayerInventory, can_stack, item_max_stack};

pub(in crate::play) const MERCHANT_MENU_TYPE_ID: i32 = 19;
pub(in crate::play) const MERCHANT_MENU_SLOT_COUNT: usize = 39;

#[derive(Debug, Clone)]
pub(in crate::play) struct MerchantWindow {
    pub(in crate::play) container_id: i32,
    pub(in crate::play) entity_id: mc_entity::EntityId,
    pub(in crate::play) customer: uuid::Uuid,
    pub(in crate::play) state_id: i32,
    pub(in crate::play) selected_offer: Option<usize>,
    pub(in crate::play) inputs: [ItemStack; 2],
    pub(in crate::play) result: ItemStack,
    pub(in crate::play) merchant: VillagerMerchantState,
}

impl MerchantWindow {
    pub(in crate::play) fn new(
        container_id: i32,
        entity_id: mc_entity::EntityId,
        customer: uuid::Uuid,
        merchant: VillagerMerchantState,
        persisted_inputs: Option<Box<[ItemStack; 2]>>,
    ) -> Self {
        Self {
            container_id,
            entity_id,
            customer,
            state_id: 1,
            selected_offer: None,
            inputs: persisted_inputs.map_or_else(
                || std::array::from_fn(|_| ItemStack::EMPTY),
                |inputs| *inputs,
            ),
            result: ItemStack::EMPTY,
            merchant,
        }
    }
}

pub(in crate::play) fn merchant_menu_title_nbt() -> Result<Vec<u8>, mc_protocol::CodecError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![(
            "translate".to_owned(),
            Tag::String("entity.minecraft.villager.toolsmith".to_owned()),
        )]),
    )?;
    Ok(out)
}

pub(in crate::play) fn merchant_input_projection(
    inputs: &[ItemStack; 2],
) -> Option<Box<[ItemStack; 2]>> {
    inputs
        .iter()
        .any(|stack| !stack.is_empty())
        .then(|| Box::new(inputs.clone()))
}

pub(in crate::play) fn merchant_input_from_projection(
    input: Option<Box<[ItemStack; 2]>>,
) -> [ItemStack; 2] {
    input.map_or_else(|| std::array::from_fn(|_| ItemStack::EMPTY), |input| *input)
}

pub(in crate::play) fn merchant_wire_items(
    window: &MerchantWindow,
    inventory: &PlayerInventory,
) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(MERCHANT_MENU_SLOT_COUNT);
    items.extend(window.inputs.iter().cloned());
    items.push(window.result.clone());
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

fn wire_item_stack(stack: &mc_entity::EntityItemStack) -> ItemStack {
    ItemStack {
        count: stack.count,
        item_id: stack.item_id,
        damage: stack.damage,
        enchantments: stack.enchantments.clone(),
        custom_name: stack.custom_name.as_deref().cloned(),
        item_model: stack
            .item_model
            .as_deref()
            .cloned()
            .map(std::sync::Arc::new),
    }
}

pub(in crate::play) fn protocol_offers(window: &MerchantWindow) -> Vec<MerchantOffer> {
    window
        .merchant
        .offers
        .iter()
        .enumerate()
        .map(|(offer_index, offer)| MerchantOffer {
            cost_a: MerchantItemCost {
                item_id: offer.cost_a.item_id,
                count: offer.cost_a.count,
            },
            result: wire_item_stack(&offer.result),
            cost_b: offer.cost_b.map(|cost| MerchantItemCost {
                item_id: cost.item_id,
                count: cost.count,
            }),
            out_of_stock: offer.is_out_of_stock(),
            uses: offer.uses,
            max_uses: offer.max_uses,
            xp: offer.xp,
            special_price: window
                .merchant
                .player_special_price(window.customer, offer_index)
                .unwrap_or(offer.special_price),
            price_multiplier: offer.price_multiplier,
            demand: offer.demand,
        })
        .collect()
}

pub(in crate::play) fn select_offer(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    window: &MerchantWindow,
    inventory: &PlayerInventory,
    selection: usize,
) -> Option<(MerchantWindow, PlayerInventory)> {
    let offer = window.merchant.offers.get(selection)?;
    if offer.is_out_of_stock() {
        return None;
    }
    let mut updated_window = window.clone();
    let mut updated_inventory = inventory.clone();
    return_inputs(
        items,
        item_facts,
        &mut updated_window,
        &mut updated_inventory,
    )?;

    let max_a = item_max_stack_for_id(item_facts, items, offer.cost_a.item_id);
    let count_a =
        window
            .merchant
            .modified_cost_a_count_for_player(window.customer, selection, max_a)?;
    updated_window.inputs[0] =
        take_cost(&mut updated_inventory, offer.cost_a.item_id, count_a, max_a)?;
    if let Some(cost_b) = offer.cost_b {
        let max_b = item_max_stack_for_id(item_facts, items, cost_b.item_id);
        updated_window.inputs[1] =
            take_cost(&mut updated_inventory, cost_b.item_id, cost_b.count, max_b)?;
    }
    updated_window.selected_offer = Some(selection);
    updated_window.result = wire_item_stack(&offer.result);
    updated_window.state_id = updated_window.state_id.wrapping_add(1);
    Some((updated_window, updated_inventory))
}

pub(in crate::play) fn refresh_selected_offer(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    window: &mut MerchantWindow,
) {
    let Some(offer_index) = window.selected_offer else {
        window.result = ItemStack::EMPTY;
        return;
    };
    let Some(offer) = window.merchant.offers.get(offer_index) else {
        window.selected_offer = None;
        window.result = ItemStack::EMPTY;
        return;
    };
    let max_a = item_max_stack_for_id(item_facts, items, offer.cost_a.item_id);
    let Some(modified_cost_a) =
        window
            .merchant
            .modified_cost_a_count_for_player(window.customer, offer_index, max_a)
    else {
        window.result = ItemStack::EMPTY;
        return;
    };
    if offer.is_out_of_stock() || !inputs_satisfy_offer(&window.inputs, offer, modified_cost_a) {
        window.result = ItemStack::EMPTY;
        return;
    }
    window.result = wire_item_stack(&offer.result);
}

fn return_inputs(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    window: &mut MerchantWindow,
    inventory: &mut PlayerInventory,
) -> Option<()> {
    for input in &mut window.inputs {
        if input.is_empty() {
            continue;
        }
        let max_stack = item_max_stack(item_facts, items, input);
        let (remaining, _) = inventory.merge_stack(std::mem::take(input), max_stack);
        if !remaining.is_empty() {
            return None;
        }
    }
    window.result = ItemStack::EMPTY;
    window.selected_offer = None;
    Some(())
}

fn take_cost(
    inventory: &mut PlayerInventory,
    item_id: u32,
    minimum_count: i32,
    maximum_count: i32,
) -> Option<ItemStack> {
    if minimum_count <= 0 || maximum_count < minimum_count {
        return None;
    }
    let mut payment = ItemStack::EMPTY;
    for slot in 9..=44 {
        let source = &mut inventory.slots[slot];
        if source.item_id != item_id
            || source.is_empty()
            || !payment.is_empty() && !can_stack(&payment, source)
        {
            continue;
        }
        if payment.is_empty() {
            payment = source.clone();
            payment.count = 0;
        }
        let transfer = source.count.min(maximum_count - payment.count);
        payment.count += transfer;
        source.count -= transfer;
        if source.count <= 0 {
            *source = ItemStack::EMPTY;
        }
        if payment.count >= maximum_count {
            break;
        }
    }
    (payment.count >= minimum_count).then_some(payment)
}

fn item_max_stack_for_id(item_facts: &ItemFactsTable, items: &ItemRegistry, item_id: u32) -> i32 {
    item_max_stack(item_facts, items, &ItemStack::new(item_id, 1))
}

pub(in crate::play) fn inputs_satisfy_offer(
    inputs: &[ItemStack; 2],
    offer: &VillagerTradeOffer,
    modified_cost_a: i32,
) -> bool {
    inputs[0].item_id == offer.cost_a.item_id
        && inputs[0].count >= modified_cost_a
        && match offer.cost_b {
            Some(cost_b) => inputs[1].item_id == cost_b.item_id && inputs[1].count >= cost_b.count,
            None => inputs[1].is_empty(),
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_entity::{EntityItemStack, villager_merchant_26_1_2::*};

    #[test]
    fn select_offer_moves_exact_cost_and_projects_result() {
        let items = mc_data::items::solaris_required_items();
        let facts = mc_data::item_components::solaris_required_item_facts();
        let emerald = items
            .id_of(&mc_data::Identifier::parse("minecraft:emerald").unwrap())
            .unwrap();
        let axe = items
            .id_of(&mc_data::Identifier::parse("minecraft:stone_axe").unwrap())
            .unwrap();
        let merchant = VillagerMerchantState::new(vec![VillagerTradeOffer::new(
            VillagerTradeCost::new(emerald, 1),
            EntityItemStack::new(axe, 1),
            12,
            1,
            0.2,
        )])
        .unwrap();
        let window = MerchantWindow::new(
            2,
            mc_entity::EntityId(7),
            uuid::Uuid::from_u128(7),
            merchant,
            None,
        );
        let mut inventory = PlayerInventory::empty();
        inventory.slots[36] = ItemStack::new(emerald, 3);

        let (selected, inventory) = select_offer(&items, &facts, &window, &inventory, 0).unwrap();
        assert_eq!(selected.inputs[0], ItemStack::new(emerald, 3));
        assert_eq!(selected.result, ItemStack::new(axe, 1));
        assert_eq!(inventory.slots[36], ItemStack::EMPTY);
    }

    #[test]
    fn select_offer_combines_matching_inventory_stacks_and_keeps_repeat_payment() {
        let items = mc_data::items::solaris_required_items();
        let facts = mc_data::item_components::solaris_required_item_facts();
        let coal = items
            .id_of(&mc_data::Identifier::parse("minecraft:coal").unwrap())
            .unwrap();
        let emerald = items
            .id_of(&mc_data::Identifier::parse("minecraft:emerald").unwrap())
            .unwrap();
        let merchant = VillagerMerchantState::new(vec![VillagerTradeOffer::new(
            VillagerTradeCost::new(coal, 15),
            EntityItemStack::new(emerald, 1),
            16,
            2,
            0.05,
        )])
        .unwrap();
        let window = MerchantWindow::new(
            2,
            mc_entity::EntityId(7),
            uuid::Uuid::from_u128(7),
            merchant,
            None,
        );
        let mut inventory = PlayerInventory::empty();
        inventory.slots[36] = ItemStack::new(coal, 8);
        inventory.slots[37] = ItemStack::new(coal, 24);

        let (mut selected, inventory) =
            select_offer(&items, &facts, &window, &inventory, 0).unwrap();
        assert_eq!(selected.inputs[0], ItemStack::new(coal, 32));
        assert_eq!(inventory.slots[36], ItemStack::EMPTY);
        assert_eq!(inventory.slots[37], ItemStack::EMPTY);

        selected.inputs[0].count -= 15;
        refresh_selected_offer(&items, &facts, &mut selected);
        assert_eq!(selected.inputs[0], ItemStack::new(coal, 17));
        assert_eq!(selected.result, ItemStack::new(emerald, 1));
    }

    #[test]
    fn protocol_and_selection_apply_only_the_current_customers_reputation() {
        let items = mc_data::items::solaris_required_items();
        let facts = mc_data::item_components::solaris_required_item_facts();
        let emerald = items
            .id_of(&mc_data::Identifier::parse("minecraft:emerald").unwrap())
            .unwrap();
        let axe = items
            .id_of(&mc_data::Identifier::parse("minecraft:stone_axe").unwrap())
            .unwrap();
        let customer = uuid::Uuid::from_u128(7);
        let mut offer = VillagerTradeOffer::new(
            VillagerTradeCost::new(emerald, 5),
            EntityItemStack::new(axe, 1),
            12,
            1,
            1.0,
        );
        offer.max_uses = 16;
        let mut merchant = VillagerMerchantState::new(vec![offer]).unwrap();
        merchant.record_player_trade(customer, 0).unwrap();
        let window = MerchantWindow::new(2, mc_entity::EntityId(7), customer, merchant, None);
        let mut inventory = PlayerInventory::empty();
        inventory.slots[36] = ItemStack::new(emerald, 3);

        assert_eq!(protocol_offers(&window)[0].special_price, -2);
        let (selected, inventory) = select_offer(&items, &facts, &window, &inventory, 0).unwrap();
        assert_eq!(selected.inputs[0], ItemStack::new(emerald, 3));
        assert_eq!(inventory.slots[36], ItemStack::EMPTY);

        let stranger = MerchantWindow::new(
            3,
            mc_entity::EntityId(7),
            uuid::Uuid::from_u128(8),
            window.merchant,
            None,
        );
        assert_eq!(protocol_offers(&stranger)[0].special_price, 0);
    }
}
