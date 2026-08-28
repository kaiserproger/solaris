use std::time::Instant;

use mc_data::ItemStack;
use mc_data::block_light::BlockLightTable;
use mc_domain::GameMode;
use mc_entity::{EntityItemStack, Vec3};

use super::explosion_authority::spawn_primed_tnt_locked;
use super::pickups::{block_item_pickup_for_owner_locked, spawn_item_drop_entity_locked};
use super::projectiles::spawn_arrow_locked;
use super::visibility::spawn_entity_visibility_locked;
use super::{SessionId, SessionRegistry};
use crate::play::BlockEdit;
use crate::play::block_edit_commit::apply_block_edit_batch_to_storage_conditionally;
use crate::play::explosions::{CommittedTntIgnition, TNT_FUSE_TICKS, TntIgnitionPlan};
use crate::play::inventory::PlayerInventory;
use crate::play::simulation::{
    BowReleasePlan, CommittedBowRelease, CommittedFoodUse, CommittedSelectedItemDrop, FoodUsePlan,
    SelectedItemDropPlan, SimulationAuthority, SimulationRequestError,
};

impl SessionRegistry {
    pub(in crate::play) fn commit_tnt_ignition(
        &self,
        _authority: &SimulationAuthority,
        storage: &mut mc_world::WorldStorage,
        block_light: Option<&BlockLightTable>,
        actor_session: SessionId,
        plan: &TntIgnitionPlan,
    ) -> Result<Option<CommittedTntIgnition>, SimulationRequestError> {
        let mut inner = self.lock_session_entities("commit TNT ignition");
        let Some(player_state) = inner.player_persistence.get(&actor_session).cloned() else {
            return Ok(None);
        };
        let mut player_state =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        if player_state.game_mode != plan.game_mode
            || !matches!(plan.game_mode, GameMode::Creative | GameMode::Survival)
        {
            return Ok(None);
        }
        let selected_slot =
            PlayerInventory::HOTBAR_BASE + usize::from(player_state.selected_hotbar_slot);
        if (plan.held_slot != PlayerInventory::OFFHAND_SLOT && plan.held_slot != selected_slot)
            || player_state.inventory.slots[plan.held_slot] != plan.expected_held
            || plan.expected_held.is_empty()
        {
            return Ok(None);
        }
        if plan.flint_and_steel_max_damage <= 0 {
            return Err(SimulationRequestError::InvalidCommand);
        }

        let mut inventory = player_state.inventory.clone();
        let mut changed_slots = Vec::new();
        if plan.game_mode == GameMode::Survival {
            let held = &mut inventory.slots[plan.held_slot];
            let new_damage = held.damage.unwrap_or(0).saturating_add(1);
            if new_damage >= plan.flint_and_steel_max_damage {
                *held = ItemStack::EMPTY;
            } else {
                held.damage = Some(new_damage);
            }
            changed_slots.push((plan.held_slot, held.clone()));
        }
        let edit = BlockEdit {
            pos: plan.tnt.pos,
            new_state: plan.air,
        };
        let Some(block) = apply_block_edit_batch_to_storage_conditionally(
            storage,
            block_light,
            std::slice::from_ref(&edit),
            std::slice::from_ref(&plan.tnt),
        ) else {
            return Ok(None);
        };
        if block.applied.len() != 1 {
            return Err(SimulationRequestError::WorldMutationFailed);
        }

        let position = Vec3::new(
            f64::from(plan.tnt.pos.x) + 0.5,
            f64::from(plan.tnt.pos.y),
            f64::from(plan.tnt.pos.z) + 0.5,
        );
        let (_, dispatches) = spawn_primed_tnt_locked(
            &mut inner,
            plan.tnt_entity_type_id,
            position,
            Vec3::new(0.0, 0.2, 0.0),
            TNT_FUSE_TICKS,
            plan.air,
        );
        if !changed_slots.is_empty() {
            player_state.replace_inventory(inventory.clone());
        }
        Ok(Some(CommittedTntIgnition {
            block,
            inventory,
            changed_slots,
            dispatches,
        }))
    }

    pub(in crate::play) fn commit_food_use(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        plan: &FoodUsePlan,
    ) -> Option<CommittedFoodUse> {
        let inner = self.lock_inner("commit food use");
        if !inner.sessions.contains_key(&actor_session) {
            return None;
        }
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit food use",
            wait_started,
            guard,
        );
        let selected_slot =
            PlayerInventory::HOTBAR_BASE + usize::from(player_state.selected_hotbar_slot);
        if (plan.held_slot != PlayerInventory::OFFHAND_SLOT && plan.held_slot != selected_slot)
            || player_state.survival != plan.expected_survival
            || !mc_entity::player_survival_26_1_2::can_eat(
                player_state.survival.health,
                player_state.survival.food,
            )
        {
            return None;
        }
        if player_state.inventory.slots[plan.held_slot] != plan.expected_held {
            return None;
        }

        let mut inventory = player_state.inventory.clone();
        let held = &mut inventory.slots[plan.held_slot];
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
        let changed_slots = vec![(plan.held_slot, held.clone())];
        let mut survival = player_state.survival;
        survival.add_food(plan.food, plan.saturation);

        player_state.replace_inventory(inventory.clone());
        player_state.survival = survival;
        Some(CommittedFoodUse {
            inventory,
            survival,
            changed_slots,
        })
    }

    pub(in crate::play) fn commit_bow_release(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        plan: &BowReleasePlan,
    ) -> Option<CommittedBowRelease> {
        let mut inner = self.lock_session_entities("commit bow release");
        if !inner.sessions.contains_key(&actor_session) {
            return None;
        }
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit bow release",
            wait_started,
            guard,
        );
        let selected_slot =
            PlayerInventory::HOTBAR_BASE + usize::from(player_state.selected_hotbar_slot);
        if plan.bow_slot != PlayerInventory::OFFHAND_SLOT && plan.bow_slot != selected_slot {
            return None;
        }
        if plan.arrow_slot >= player_state.inventory.slots.len()
            || player_state.inventory.slots[plan.bow_slot] != plan.expected_bow
            || player_state.inventory.slots[plan.arrow_slot] != plan.expected_arrow
        {
            return None;
        }

        let mut inventory = player_state.inventory.clone();
        let arrow = &mut inventory.slots[plan.arrow_slot];
        arrow.count = arrow.count.saturating_sub(1);
        if arrow.count <= 0 {
            *arrow = ItemStack::EMPTY;
        }
        let mut changed_slots = vec![(plan.arrow_slot, arrow.clone())];

        let bow = &mut inventory.slots[plan.bow_slot];
        let new_damage = bow.damage.unwrap_or(0).saturating_add(1);
        if new_damage >= plan.bow_max_damage {
            *bow = ItemStack::EMPTY;
        } else {
            bow.damage = Some(new_damage);
        }
        changed_slots.push((plan.bow_slot, bow.clone()));

        player_state.replace_inventory(inventory.clone());
        let (_, dispatches) = spawn_arrow_locked(
            &mut inner,
            Some(actor_session),
            plan.entity_type_id,
            plan.position,
            plan.velocity,
            plan.rotation,
        );
        Some(CommittedBowRelease {
            inventory,
            changed_slots,
            dispatches,
        })
    }

    pub(in crate::play) fn commit_selected_item_drop(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        plan: &SelectedItemDropPlan,
    ) -> Option<CommittedSelectedItemDrop> {
        let mut inner = self.lock_session_entities("commit selected item drop");
        if !inner.sessions.contains_key(&actor_session) {
            return None;
        }
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit selected item drop",
            wait_started,
            guard,
        );
        if player_state.selected_hotbar_slot != plan.held_hotbar_slot {
            return None;
        }
        let held_slot = PlayerInventory::HOTBAR_BASE + usize::from(plan.held_hotbar_slot);
        if player_state.inventory.slots[held_slot] != plan.expected_held {
            return None;
        }

        let mut inventory = player_state.inventory.clone();
        let held = &mut inventory.slots[held_slot];
        let dropped = EntityItemStack {
            item_id: held.item_id,
            count: plan.drop_count,
            damage: held.damage,
            enchantments: held.enchantments.clone(),
            custom_name: held.custom_name.clone().map(Box::new),
            item_model: held.item_model.as_deref().cloned().map(Box::new),
        };
        held.count -= plan.drop_count;
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
        let changed_slots = vec![(held_slot, held.clone())];

        let entity_id =
            spawn_item_drop_entity_locked(&mut inner, plan.entity_type_id, plan.position, dropped)?;
        block_item_pickup_for_owner_locked(&mut inner, entity_id, actor_session);
        let dispatches = spawn_entity_visibility_locked(&mut inner, entity_id);
        player_state.replace_inventory(inventory.clone());

        Some(CommittedSelectedItemDrop {
            inventory,
            changed_slots,
            dispatches,
        })
    }
}
