use super::*;

pub(super) async fn open_merchant_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
    entity_id: EntityId,
    player_pose: PlayerPose,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(merchant) = state.sessions.villager_merchant_snapshot(entity_id) else {
        return Ok(false);
    };
    store_active_container(state, player_pose).await?;
    let Some((inventory, carried_item, merchant_input)) = state
        .sessions
        .player_merchant_container_state(state.session_id)
    else {
        return Ok(false);
    };
    state.inventory = inventory;
    state.carried_item = carried_item;
    let window = MerchantWindow::new(
        next_container_id(state),
        entity_id,
        merchant,
        merchant_input,
    );
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id: window.container_id,
            menu_type: MERCHANT_MENU_TYPE_ID,
            title_nbt: merchant_menu_title_nbt()?,
        },
        state.compression,
    )
    .await?;
    write_merchant_window(state, writer, &window).await?;
    state.active_container = Some(ActiveContainer::Merchant(window));
    Ok(true)
}

pub(super) async fn handle_select_trade<W>(
    state: &mut InteractionState,
    writer: &mut W,
    packet: ServerboundSelectTrade,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(active) = state.active_container.take() else {
        return Ok(());
    };
    let ActiveContainer::Merchant(mut window) = active else {
        state.active_container = Some(active);
        return Ok(());
    };
    let Some(current_merchant) = state.sessions.villager_merchant_snapshot(window.entity_id) else {
        state.active_container = Some(ActiveContainer::Merchant(window));
        return Ok(());
    };
    if current_merchant != window.merchant {
        window.merchant = current_merchant;
        window.selected_offer = None;
        window.result = ItemStack::EMPTY;
        write_merchant_window(state, writer, &window).await?;
        state.active_container = Some(ActiveContainer::Merchant(window));
        return Ok(());
    }
    let Ok(selection) = usize::try_from(packet.offer_index) else {
        write_merchant_window(state, writer, &window).await?;
        state.active_container = Some(ActiveContainer::Merchant(window));
        return Ok(());
    };
    let Some((planned_window, updated_inventory)) = select_merchant_offer(
        &state.items,
        &state.item_facts,
        &window,
        &state.inventory,
        selection,
    ) else {
        write_merchant_window(state, writer, &window).await?;
        state.active_container = Some(ActiveContainer::Merchant(window));
        return Ok(());
    };
    let plan = ContainerPlayerPlan {
        expected_inventory: state.inventory.clone(),
        expected_carried_item: state.carried_item.clone(),
        updated_inventory,
        updated_carried_item: state.carried_item.clone(),
        crafting_table_input: None,
        enchanting_table_input: None,
        merchant_input: Some(MerchantInputPlan {
            expected: merchant_input_projection(&window.inputs),
            updated: merchant_input_projection(&planned_window.inputs),
        }),
        drops: Vec::new(),
        xp_orb: None,
    };
    match state.simulation.commit_player_inventory(plan).await {
        Ok(PlayerInventoryCommitOutcome::Committed {
            inventory,
            carried_item,
            merchant_input,
            ..
        }) => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            window = planned_window;
            window.inputs = merchant_input_from_projection(merchant_input);
        }
        Ok(PlayerInventoryCommitOutcome::Rejected {
            inventory,
            carried_item,
            merchant_input,
            ..
        }) => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            window.inputs = merchant_input_from_projection(merchant_input);
            window.selected_offer = None;
            window.result = ItemStack::EMPTY;
        }
        Err(error) => {
            state.active_container = Some(ActiveContainer::Merchant(window));
            debug!(?error, "merchant selection owner commit rejected");
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "selecting merchant trade",
            });
        }
    }
    write_merchant_window(state, writer, &window).await?;
    state.active_container = Some(ActiveContainer::Merchant(window));
    Ok(())
}

pub(super) async fn handle_merchant_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: MerchantWindow,
    packet: ServerboundContainerClick,
) -> Result<MerchantWindow, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let destination = match classify_container_click(&packet) {
        ContainerClickAction::Pickup {
            slot: 2,
            button: 0 | 1,
        } => Some(MerchantTradeDestination::Cursor),
        ContainerClickAction::QuickMove { slot: 2 } => Some(MerchantTradeDestination::Inventory),
        _ => None,
    };
    if packet.container_id != window.container_id
        || packet.state_id != window.state_id
        || destination.is_none()
    {
        write_merchant_window(state, writer, &window).await?;
        return Ok(window);
    }

    let Some(offer_index) = window.selected_offer else {
        write_merchant_window(state, writer, &window).await?;
        return Ok(window);
    };
    let Some(current_merchant) = state.sessions.villager_merchant_snapshot(window.entity_id) else {
        write_merchant_window(state, writer, &window).await?;
        return Ok(window);
    };
    if current_merchant != window.merchant {
        window.merchant = current_merchant;
        window.selected_offer = None;
        window.result = ItemStack::EMPTY;
        write_merchant_window(state, writer, &window).await?;
        return Ok(window);
    }
    let Some(offer) = window.merchant.offers.get(offer_index) else {
        write_merchant_window(state, writer, &window).await?;
        return Ok(window);
    };
    if offer.is_out_of_stock() || window.result.is_empty() {
        write_merchant_window(state, writer, &window).await?;
        return Ok(window);
    }
    let destination = destination.expect("merchant destination checked");
    let cost_a_max_stack = item_max_stack(
        &state.item_facts,
        &state.items,
        &ItemStack::new(offer.cost_a.item_id, 1),
    );
    let result_max_stack = item_max_stack(&state.item_facts, &state.items, &window.result);
    let predicted_carried_item = match destination {
        MerchantTradeDestination::Cursor if state.carried_item.is_empty() => window.result.clone(),
        MerchantTradeDestination::Cursor
            if can_stack(&state.carried_item, &window.result)
                && state
                    .carried_item
                    .count
                    .checked_add(window.result.count)
                    .is_some_and(|count| count <= result_max_stack) =>
        {
            let mut carried_item = state.carried_item.clone();
            carried_item.count += window.result.count;
            carried_item
        }
        MerchantTradeDestination::Cursor => {
            write_merchant_window(state, writer, &window).await?;
            return Ok(window);
        }
        MerchantTradeDestination::Inventory => state.carried_item.clone(),
    };
    if !client_carried_item_matches(&packet.carried_item, &predicted_carried_item) {
        write_merchant_window(state, writer, &window).await?;
        return Ok(window);
    }
    let plan = MerchantTradePlan {
        entity_id: window.entity_id,
        expected_merchant: window.merchant.clone(),
        offer_index,
        expected_inventory: state.inventory.clone(),
        expected_carried_item: state.carried_item.clone(),
        expected_merchant_input: merchant_input_projection(&window.inputs),
        destination,
        cost_a_max_stack,
        result_max_stack,
    };
    match state.simulation.commit_merchant_trade(plan).await {
        Ok(Some(committed)) => {
            state.inventory = committed.inventory;
            state.carried_item = committed.carried_item;
            window.inputs = merchant_input_from_projection(committed.merchant_input);
            window.merchant = committed.merchant;
            super::containers::refresh_selected_offer(&state.items, &state.item_facts, &mut window);
            window.state_id = window.state_id.wrapping_add(1);
        }
        Ok(None) => {
            if let Some((inventory, carried_item, merchant_input)) = state
                .sessions
                .player_merchant_container_state(state.session_id)
            {
                state.inventory = inventory;
                state.carried_item = carried_item;
                window.inputs = merchant_input_from_projection(merchant_input);
            }
            if let Some(merchant) = state.sessions.villager_merchant_snapshot(window.entity_id) {
                window.merchant = merchant;
            }
            window.selected_offer = None;
            window.result = ItemStack::EMPTY;
        }
        Err(error) => {
            debug!(?error, "merchant trade owner commit rejected");
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "committing merchant trade",
            });
        }
    }
    write_merchant_window(state, writer, &window).await?;
    Ok(window)
}
