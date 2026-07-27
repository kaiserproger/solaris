use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use mc_entity::villager_merchant_26_1_2::{
    VillagerMerchantState, VillagerTradeCost, VillagerTradeOffer,
};
use mc_entity::{
    EntityItemStack, EntityLifecycle, SpawnEntity, Vec3, VillagerData, VillagerKind,
    VillagerProfession,
};
use mc_protocol::packets::play::{GameMode, ItemStack};
use tokio::sync::mpsc;

use super::*;
use crate::login::LoggedInProfile;
use crate::play::persistence::PlayerPersistedState;
use crate::play::simulation::{MerchantTradeDestination, MerchantTradePlan, SimulationAuthority};

fn register_player(
    registry: &SessionRegistry,
    name: &str,
) -> (SessionId, Arc<Mutex<PlayerPersistedState>>) {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (outbound, _receiver) = mpsc::channel(8);
    let session = registry
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::new(),
            outbound,
            PlayerPose::new(0.5, 64.0, 0.5),
        )
        .0;
    let state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 0.5),
    )));
    registry.register_player_persistence(session, Arc::clone(&state));
    (session, state)
}

fn merchant(emerald: u32, axe: u32) -> VillagerMerchantState {
    VillagerMerchantState::new(vec![VillagerTradeOffer::new(
        VillagerTradeCost::new(emerald, 1),
        EntityItemStack::new(axe, 1),
        12,
        1,
        0.2,
    )])
    .unwrap()
}

fn spawn_merchant(
    registry: &SessionRegistry,
    merchant: VillagerMerchantState,
) -> mc_entity::EntityId {
    let mut entities = registry.lock_entities("spawn merchant authority fixture");
    let mut spawn = SpawnEntity::new(139, "minecraft:villager", Vec3::new(1.5, 64.0, 0.5));
    spawn.retained.villager = Some(VillagerData::new(
        VillagerKind::Plains,
        VillagerProfession::Toolsmith,
        1,
    ));
    spawn.retained.villager_merchant = Some(merchant);
    entities.spawn(spawn)
}

fn trade_plan(
    entity_id: mc_entity::EntityId,
    merchant: VillagerMerchantState,
    inventory: crate::play::inventory::PlayerInventory,
    carried_item: ItemStack,
    inputs: [ItemStack; 2],
    destination: MerchantTradeDestination,
) -> MerchantTradePlan {
    MerchantTradePlan {
        entity_id,
        expected_merchant: merchant,
        offer_index: 0,
        expected_inventory: inventory,
        expected_carried_item: carried_item,
        expected_merchant_input: Some(Box::new(inputs)),
        destination,
        cost_a_max_stack: 64,
        result_max_stack: 1,
    }
}

#[test]
fn merchant_trade_commits_player_and_villager_once_and_rejects_stale_replay() {
    let registry = SessionRegistry::new();
    let authority = SimulationAuthority::for_test();
    let (session, state) = register_player(&registry, "MerchantOwner");
    let emerald = 17;
    let axe = 23;
    let merchant = merchant(emerald, axe);
    let entity_id = spawn_merchant(&registry, merchant.clone());
    let inventory = crate::play::inventory::PlayerInventory::empty();
    let inputs = [ItemStack::new(emerald, 1), ItemStack::EMPTY];
    {
        let mut player = state.lock().unwrap();
        player.game_mode = GameMode::Survival;
        player.inventory = inventory.clone();
        player.merchant_input = Some(Box::new(inputs.clone()));
    }
    let plan = trade_plan(
        entity_id,
        merchant.clone(),
        inventory,
        ItemStack::EMPTY,
        inputs,
        MerchantTradeDestination::Cursor,
    );

    let committed = registry
        .commit_merchant_trade(&authority, session, &plan)
        .expect("merchant trade commits");
    assert_eq!(committed.carried_item, ItemStack::new(axe, 1));
    assert_eq!(committed.merchant_input, None);
    assert_eq!(committed.merchant.offers[0].uses, 1);
    assert_eq!(committed.merchant.xp, 1);
    assert_eq!(
        committed
            .merchant
            .trading_reputation(crate::login::offline_uuid("MerchantOwner")),
        2
    );
    let snapshot = registry
        .lock_entities("read committed merchant")
        .snapshot(entity_id)
        .unwrap();
    assert_eq!(snapshot.lifecycle, EntityLifecycle::Alive);
    assert_eq!(
        snapshot.retained.villager_merchant.as_ref().unwrap().offers[0].uses,
        1
    );
    let player = state.lock().unwrap();
    assert_eq!(player.carried_item, ItemStack::new(axe, 1));
    assert_eq!(player.merchant_input, None);
    drop(player);

    assert!(
        registry
            .commit_merchant_trade(&authority, session, &plan)
            .is_none()
    );
    let replayed = registry
        .lock_entities("read replay merchant")
        .snapshot(entity_id)
        .unwrap();
    let replayed = replayed.retained.villager_merchant.as_ref().unwrap();
    assert_eq!(replayed.offers[0].uses, 1);
    assert_eq!(
        replayed.trading_reputation(crate::login::offline_uuid("MerchantOwner")),
        2,
        "stale replay must not duplicate reputation"
    );
}

#[test]
fn merchant_inventory_trade_keeps_payment_remainder_for_repeated_quick_move() {
    let registry = SessionRegistry::new();
    let authority = SimulationAuthority::for_test();
    let (session, state) = register_player(&registry, "MerchantRepeat");
    let coal = 17;
    let emerald = 23;
    let merchant = VillagerMerchantState::new(vec![VillagerTradeOffer::new(
        VillagerTradeCost::new(coal, 15),
        EntityItemStack::new(emerald, 1),
        16,
        2,
        0.05,
    )])
    .unwrap();
    let entity_id = spawn_merchant(&registry, merchant.clone());
    let inventory = crate::play::inventory::PlayerInventory::empty();
    let inputs = [ItemStack::new(coal, 32), ItemStack::EMPTY];
    {
        let mut player = state.lock().unwrap();
        player.inventory = inventory.clone();
        player.merchant_input = Some(Box::new(inputs.clone()));
    }

    let first = registry
        .commit_merchant_trade(
            &authority,
            session,
            &MerchantTradePlan {
                entity_id,
                expected_merchant: merchant,
                offer_index: 0,
                expected_inventory: inventory,
                expected_carried_item: ItemStack::EMPTY,
                expected_merchant_input: Some(Box::new(inputs)),
                destination: MerchantTradeDestination::Inventory,
                cost_a_max_stack: 64,
                result_max_stack: 64,
            },
        )
        .expect("first repeated merchant trade commits");
    assert_eq!(
        first.merchant_input.as_deref().unwrap()[0],
        ItemStack::new(coal, 17)
    );
    assert_eq!(first.merchant.offers[0].uses, 1);
    assert_eq!(first.merchant.xp, 2);
    assert_eq!(
        first
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == emerald)
            .map(|stack| stack.count)
            .sum::<i32>(),
        1
    );

    let second = registry
        .commit_merchant_trade(
            &authority,
            session,
            &MerchantTradePlan {
                entity_id,
                expected_merchant: first.merchant.clone(),
                offer_index: 0,
                expected_inventory: first.inventory.clone(),
                expected_carried_item: first.carried_item.clone(),
                expected_merchant_input: first.merchant_input.clone(),
                destination: MerchantTradeDestination::Inventory,
                cost_a_max_stack: 64,
                result_max_stack: 64,
            },
        )
        .expect("second repeated merchant trade commits");
    assert_eq!(
        second.merchant_input.as_deref().unwrap()[0],
        ItemStack::new(coal, 2)
    );
    assert_eq!(second.merchant.offers[0].uses, 2);
    assert_eq!(second.merchant.xp, 4);
    assert_eq!(
        second
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == emerald)
            .map(|stack| stack.count)
            .sum::<i32>(),
        2
    );

    assert!(
        registry
            .commit_merchant_trade(
                &authority,
                session,
                &MerchantTradePlan {
                    entity_id,
                    expected_merchant: second.merchant,
                    offer_index: 0,
                    expected_inventory: second.inventory,
                    expected_carried_item: second.carried_item,
                    expected_merchant_input: second.merchant_input,
                    destination: MerchantTradeDestination::Inventory,
                    cost_a_max_stack: 64,
                    result_max_stack: 64,
                },
            )
            .is_none(),
        "payment remainder below the current price must not produce a third result"
    );
}

#[test]
fn merchant_trade_rejects_incompatible_cursor_and_out_of_stock_without_mutation() {
    let registry = SessionRegistry::new();
    let authority = SimulationAuthority::for_test();
    let (session, state) = register_player(&registry, "MerchantReject");
    let emerald = 17;
    let axe = 23;
    let dirt = 1;
    let merchant = merchant(emerald, axe);
    let entity_id = spawn_merchant(&registry, merchant.clone());
    let inventory = crate::play::inventory::PlayerInventory::empty();
    let inputs = [ItemStack::new(emerald, 1), ItemStack::EMPTY];
    {
        let mut player = state.lock().unwrap();
        player.inventory = inventory.clone();
        player.carried_item = ItemStack::new(dirt, 1);
        player.merchant_input = Some(Box::new(inputs.clone()));
    }
    let incompatible = trade_plan(
        entity_id,
        merchant.clone(),
        inventory.clone(),
        ItemStack::new(dirt, 1),
        inputs.clone(),
        MerchantTradeDestination::Cursor,
    );
    assert!(
        registry
            .commit_merchant_trade(&authority, session, &incompatible)
            .is_none()
    );

    let mut exhausted = merchant.clone();
    exhausted.offers[0].uses = exhausted.offers[0].max_uses;
    {
        let mut entities = registry.lock_entities("install exhausted merchant");
        let expected = entities.snapshot(entity_id).unwrap();
        let mut next = expected.clone();
        next.retained.villager_merchant = Some(exhausted.clone());
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    {
        let mut player = state.lock().unwrap();
        player.carried_item = ItemStack::EMPTY;
    }
    let exhausted_plan = trade_plan(
        entity_id,
        exhausted,
        inventory,
        ItemStack::EMPTY,
        inputs,
        MerchantTradeDestination::Cursor,
    );
    assert!(
        registry
            .commit_merchant_trade(&authority, session, &exhausted_plan)
            .is_none()
    );
    let player = state.lock().unwrap();
    assert_eq!(player.carried_item, ItemStack::EMPTY);
    assert_eq!(
        player.merchant_input.as_deref().unwrap()[0],
        ItemStack::new(emerald, 1)
    );
}
