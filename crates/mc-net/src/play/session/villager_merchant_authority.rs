use std::time::Instant;

use mc_data::ItemStack;
use mc_domain::GameMode;
use mc_entity::{EntityId, EntityLifecycle};

use crate::play::containers::inputs_satisfy_offer;
use crate::play::inventory::can_stack;
use crate::play::simulation::{
    CommittedMerchantTrade, MerchantTradeDestination, MerchantTradePlan, SimulationAuthority,
};

use super::interaction_geometry::{entity_geometry, within_entity_reach};
use super::{
    OutboundCommand, SessionId, SessionRegistry, record_entity_dispatches_locked,
    server_entity_snapshot_from, session_recipients, visibility_dispatches,
    visible_entity_observers_locked,
};

impl SessionRegistry {
    pub(in crate::play) fn session_player_uuid(&self, session_id: SessionId) -> Option<uuid::Uuid> {
        self.lock_inner("read merchant customer uuid")
            .sessions
            .get(&session_id)
            .map(|session| session.uuid)
    }

    pub(in crate::play) fn villager_merchant_snapshot(
        &self,
        entity_id: EntityId,
    ) -> Option<(
        mc_entity::villager_merchant_26_1_2::VillagerMerchantState,
        mc_entity::villager_gossip_26_1_2::VillagerGossipState,
    )> {
        let entities = self.lock_entities("read villager merchant snapshot");
        let snapshot = entities.snapshot(entity_id)?;
        if snapshot.lifecycle != EntityLifecycle::Alive
            || snapshot.type_name != "minecraft:villager"
        {
            return None;
        }
        Some((
            snapshot.retained.villager_merchant?,
            snapshot.retained.villager_gossip.unwrap_or_default(),
        ))
    }

    pub(in crate::play) fn commit_merchant_trade(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        plan: &MerchantTradePlan,
    ) -> Option<CommittedMerchantTrade> {
        let mut inner = self.lock_session_entities("commit villager merchant trade");
        let (player_pose, player_uuid) = inner
            .sessions
            .get(&actor_session)
            .map(|session| (session.pose, session.uuid))?;
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit villager merchant trade",
            wait_started,
            guard,
        );
        if player_state.game_mode == GameMode::Spectator
            || player_state.survival.is_dead()
            || player_state.inventory.slots != plan.expected_inventory.slots
            || player_state.carried_item != plan.expected_carried_item
            || player_state.merchant_input != plan.expected_merchant_input
        {
            return None;
        }

        let entity = inner.entities.snapshot(plan.entity_id)?;
        let current_gossip = entity.retained.villager_gossip.clone().unwrap_or_default();
        if entity.lifecycle != EntityLifecycle::Alive
            || entity.type_name != "minecraft:villager"
            || entity.retained.villager_merchant.as_ref() != Some(&plan.expected_merchant)
            || current_gossip != plan.expected_gossip
            || !within_entity_reach(
                player_pose,
                entity.position,
                entity_geometry(&entity.type_name, entity.animal).aabb,
                player_state.game_mode,
            )
        {
            return None;
        }
        let mut villager = entity.retained.villager?;
        let offer = plan.expected_merchant.offers.get(plan.offer_index)?;
        let reputation = plan.expected_gossip.player_reputation(player_uuid);
        let modified_cost_a =
            offer.modified_cost_a_count_for_reputation(plan.cost_a_max_stack, reputation);
        let mut inputs = *plan.expected_merchant_input.clone()?;
        if !inputs_satisfy_offer(&inputs, offer, modified_cost_a) {
            return None;
        }

        let mut merchant = plan.expected_merchant.clone();
        let (result, _) = merchant.record_trade(plan.offer_index).ok()?;
        let mut gossip = plan.expected_gossip.clone();
        gossip.record_event(
            mc_entity::villager_gossip_26_1_2::VillagerGossipEvent::Trade {
                player: player_uuid,
            },
        );
        let result = wire_item_stack(&result);
        let mut inventory = plan.expected_inventory.clone();
        let mut carried_item = plan.expected_carried_item.clone();
        match plan.destination {
            MerchantTradeDestination::Cursor => {
                if carried_item.is_empty() {
                    carried_item = result;
                } else if can_stack(&carried_item, &result)
                    && carried_item
                        .count
                        .checked_add(result.count)
                        .is_some_and(|count| count <= plan.result_max_stack)
                {
                    carried_item.count += result.count;
                } else {
                    return None;
                }
            }
            MerchantTradeDestination::Inventory => {
                let (remaining, _) = inventory.merge_stack(result, plan.result_max_stack);
                if !remaining.is_empty() {
                    return None;
                }
            }
        }

        debit(&mut inputs[0], modified_cost_a)?;
        if let Some(cost_b) = offer.cost_b {
            debit(&mut inputs[1], cost_b.count)?;
        }
        let merchant_input = inputs
            .iter()
            .any(|stack| !stack.is_empty())
            .then(|| Box::new(inputs));

        let previous_level = villager.level;
        villager.level = merchant.level();
        let mut next = entity.clone();
        next.retained.villager = Some(villager);
        next.retained.villager_gossip = Some(gossip.clone());
        next.retained.villager_merchant = Some(merchant.clone());
        let published = server_entity_snapshot_from(next.clone());
        if !inner.entities.replace_snapshot_if_current(entity, next) {
            return None;
        }
        inner
            .published_entity_snapshots
            .insert(plan.entity_id, published.clone());

        player_state.replace_container(inventory.clone(), carried_item.clone());
        player_state.merchant_input = merchant_input.clone();

        let mut dispatches = if villager.level != previous_level {
            let recipients = session_recipients(
                &inner,
                visible_entity_observers_locked(&inner, plan.entity_id),
            );
            visibility_dispatches(recipients, || {
                OutboundCommand::UpdateEntityData(published.clone())
            })
        } else {
            Vec::new()
        };
        record_entity_dispatches_locked(&mut inner, &dispatches);

        Some(CommittedMerchantTrade {
            inventory,
            carried_item,
            merchant_input,
            merchant,
            gossip,
            dispatches: std::mem::take(&mut dispatches),
        })
    }
}

fn debit(stack: &mut ItemStack, count: i32) -> Option<()> {
    if count <= 0 || stack.count < count {
        return None;
    }
    stack.count -= count;
    if stack.count <= 0 {
        *stack = ItemStack::EMPTY;
    }
    Some(())
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
