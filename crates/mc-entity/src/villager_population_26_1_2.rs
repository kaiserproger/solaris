//! Vanilla 26.1.2 villager population, food, and inventory facts.
//!
//! Confirmed local oracle:
//! - `Villager.FOOD_POINTS`: bread `4`, potato/carrot/beetroot `1`;
//! - breeding willingness is `foodLevel + inventory food points >= 12`;
//! - villagers own an eight-slot `SimpleContainer`;
//! - at the birth tick each parent eats inventory food until `foodLevel >= 12`,
//!   then digests exactly `12` points;
//! - courtship delay is `275 + nextInt(50)`, parent cooldown is `6000`,
//!   and baby age starts at `-24000`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::EntityItemStack;

pub const VILLAGER_INVENTORY_SLOTS: usize = 8;
pub const BREAD_FOOD_POINTS: u16 = 4;
pub const ROOT_CROP_FOOD_POINTS: u16 = 1;
pub const VILLAGER_BREEDING_WILLINGNESS_POINTS: u16 = 12;
pub const VILLAGER_EXCESS_FOOD_POINTS: u16 = 24;
pub const VILLAGER_BIRTH_DELAY_MIN_TICKS: u16 = 275;
pub const VILLAGER_BIRTH_DELAY_RANDOM_BOUND: u16 = 50;
pub const VILLAGER_COURTSHIP_SPEED: f64 = 0.3;
pub const VILLAGER_PARENT_COOLDOWN_TICKS: i32 = 6_000;
pub const VILLAGER_BABY_START_AGE_TICKS: i32 = -24_000;
pub const VILLAGER_BREAD_SEARCH_RADIUS: f64 = 3.0;
pub const VILLAGER_ITEM_PICKUP_HORIZONTAL_REACH: f64 = 1.0;
pub const VILLAGER_ITEM_PICKUP_VERTICAL_REACH: f64 = 0.0;
pub const VILLAGER_SHARED_FOOD_PICKUP_RADIUS: f64 = 2.0;
pub const VILLAGER_ITEM_THROW_Y_OFFSET: f64 = 1.32;
pub const VILLAGER_ITEM_THROW_SPEED: f64 = 0.3;
pub const VILLAGER_ITEM_THROW_PICKUP_DELAY_TICKS: u64 = 10;
pub const VILLAGER_INTERACTION_RANGE: f64 = 8.0;
pub const VILLAGER_COURTSHIP_DISTANCE_SQUARED: f64 = 5.0;
pub const VILLAGER_HOME_SEARCH_RADIUS: f64 = 48.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VillagerFoodItemIds {
    pub bread: u32,
    pub potato: u32,
    pub carrot: u32,
    pub beetroot: u32,
}

impl VillagerFoodItemIds {
    #[must_use]
    pub const fn food_points(self, item_id: u32) -> Option<u16> {
        if item_id == self.bread {
            Some(BREAD_FOOD_POINTS)
        } else if item_id == self.potato || item_id == self.carrot || item_id == self.beetroot {
            Some(ROOT_CROP_FOOD_POINTS)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn contains(self, item_id: u32) -> bool {
        self.food_points(item_id).is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VillagerInventory {
    slots: [Option<EntityItemStack>; VILLAGER_INVENTORY_SLOTS],
}

impl Default for VillagerInventory {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
        }
    }
}

impl VillagerInventory {
    #[must_use]
    pub const fn slots(&self) -> &[Option<EntityItemStack>; VILLAGER_INVENTORY_SLOTS] {
        &self.slots
    }

    #[must_use]
    pub fn count_item(&self, item_id: u32) -> i32 {
        self.slots
            .iter()
            .filter_map(Option::as_ref)
            .filter(|stack| stack.item_id == item_id)
            .map(|stack| stack.count.max(0))
            .sum()
    }

    #[must_use]
    pub fn food_points(&self, food_items: VillagerFoodItemIds) -> u16 {
        self.slots
            .iter()
            .filter_map(Option::as_ref)
            .filter_map(|stack| {
                food_items.food_points(stack.item_id).map(|points| {
                    u32::from(points) * u32::try_from(stack.count.max(0)).unwrap_or(0)
                })
            })
            .sum::<u32>()
            .min(u32::from(u16::MAX)) as u16
    }

    #[must_use]
    pub fn can_add_stack(&self, stack: &EntityItemStack, max_stack_size: i32) -> bool {
        if stack.count <= 0 || max_stack_size <= 0 {
            return false;
        }
        self.slots.iter().any(|slot| match slot {
            None => true,
            Some(existing) => {
                same_item_and_components(existing, stack) && existing.count < max_stack_size
            }
        })
    }

    pub fn add_stack(
        &mut self,
        mut stack: EntityItemStack,
        max_stack_size: i32,
    ) -> Result<Option<EntityItemStack>, VillagerPopulationError> {
        if stack.count <= 0 || !(1..=i32::from(u8::MAX)).contains(&max_stack_size) {
            return Err(VillagerPopulationError::InvalidStack);
        }

        for slot in &mut self.slots {
            let Some(existing) = slot.as_mut() else {
                continue;
            };
            if !same_item_and_components(existing, &stack) || existing.count >= max_stack_size {
                continue;
            }
            let moved = (max_stack_size - existing.count).min(stack.count);
            existing.count += moved;
            stack.count -= moved;
            if stack.count == 0 {
                return Ok(None);
            }
        }

        for slot in &mut self.slots {
            if slot.is_some() {
                continue;
            }
            let moved = max_stack_size.min(stack.count);
            let mut inserted = stack.clone();
            inserted.count = moved;
            *slot = Some(inserted);
            stack.count -= moved;
            if stack.count == 0 {
                return Ok(None);
            }
        }

        Ok(Some(stack))
    }

    pub fn eat_until_full(
        &mut self,
        food_level: &mut u16,
        food_items: VillagerFoodItemIds,
    ) -> Result<(), VillagerPopulationError> {
        if *food_level >= VILLAGER_BREEDING_WILLINGNESS_POINTS {
            return Ok(());
        }

        for slot in &mut self.slots {
            let Some(stack) = slot.as_mut() else {
                continue;
            };
            let Some(points) = food_items.food_points(stack.item_id) else {
                continue;
            };
            while stack.count > 0 && *food_level < VILLAGER_BREEDING_WILLINGNESS_POINTS {
                *food_level = food_level
                    .checked_add(points)
                    .ok_or(VillagerPopulationError::FoodOverflow)?;
                stack.count -= 1;
            }
            if stack.count == 0 {
                *slot = None;
            }
            if *food_level >= VILLAGER_BREEDING_WILLINGNESS_POINTS {
                return Ok(());
            }
        }

        Err(VillagerPopulationError::NotWilling)
    }

    pub fn extract_food_share(
        &mut self,
        food_items: VillagerFoodItemIds,
        max_stack_size: i32,
    ) -> Option<EntityItemStack> {
        if self.food_points(food_items) < VILLAGER_EXCESS_FOOD_POINTS || max_stack_size <= 0 {
            return None;
        }
        for slot in &mut self.slots {
            let Some(stack) = slot.as_mut() else {
                continue;
            };
            if !food_items.contains(stack.item_id) {
                continue;
            }
            let count = if stack.count > max_stack_size / 2 {
                stack.count / 2
            } else if stack.count > 24 {
                stack.count - 24
            } else {
                0
            };
            if count <= 0 {
                continue;
            }
            stack.count -= count;
            let mut shared = stack.clone();
            shared.count = count;
            if stack.count == 0 {
                *slot = None;
            }
            return Some(shared);
        }
        None
    }
}

fn same_item_and_components(left: &EntityItemStack, right: &EntityItemStack) -> bool {
    left.item_id == right.item_id
        && left.damage == right.damage
        && left.enchantments == right.enchantments
        && left.custom_name == right.custom_name
        && left.item_model == right.item_model
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VillagerPendingBirth {
    pub partner_uuid: Uuid,
    pub started_tick: u64,
    pub ready_tick: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VillagerPopulationState {
    pub age_ticks: i32,
    pub food_level: u16,
    pub inventory: VillagerInventory,
    pub pending_birth: Option<VillagerPendingBirth>,
    pub claimed_home: Option<String>,
}

impl VillagerPopulationState {
    #[must_use]
    pub fn adult() -> Self {
        Self {
            age_ticks: 0,
            food_level: 0,
            inventory: VillagerInventory::default(),
            pending_birth: None,
            claimed_home: None,
        }
    }

    #[must_use]
    pub fn baby(claimed_home: String) -> Self {
        Self {
            age_ticks: VILLAGER_BABY_START_AGE_TICKS,
            food_level: 0,
            inventory: VillagerInventory::default(),
            pending_birth: None,
            claimed_home: Some(claimed_home),
        }
    }

    #[must_use]
    pub fn total_food_points(&self, food_items: VillagerFoodItemIds) -> u16 {
        self.food_level
            .saturating_add(self.inventory.food_points(food_items))
    }

    #[must_use]
    pub fn can_breed(&self, sleeping: bool, food_items: VillagerFoodItemIds) -> bool {
        !sleeping
            && self.age_ticks == 0
            && self.total_food_points(food_items) >= VILLAGER_BREEDING_WILLINGNESS_POINTS
            && self.pending_birth.is_none()
    }

    #[must_use]
    pub fn has_excess_food(&self, food_items: VillagerFoodItemIds) -> bool {
        self.inventory.food_points(food_items) >= VILLAGER_EXCESS_FOOD_POINTS
    }

    #[must_use]
    pub fn wants_more_food(&self, food_items: VillagerFoodItemIds) -> bool {
        self.inventory.food_points(food_items) < VILLAGER_BREEDING_WILLINGNESS_POINTS
    }

    pub fn add_to_inventory(
        &mut self,
        stack: EntityItemStack,
        max_stack_size: i32,
    ) -> Result<Option<EntityItemStack>, VillagerPopulationError> {
        self.inventory.add_stack(stack, max_stack_size)
    }

    pub fn start_pending_birth(
        &mut self,
        partner_uuid: Uuid,
        started_tick: u64,
        deterministic_seed: u64,
        sleeping: bool,
        food_items: VillagerFoodItemIds,
    ) -> Result<u64, VillagerPopulationError> {
        if !self.can_breed(sleeping, food_items) {
            return Err(VillagerPopulationError::NotWilling);
        }
        let ready_tick = started_tick
            .checked_add(u64::from(villager_birth_delay_ticks(deterministic_seed)))
            .ok_or(VillagerPopulationError::TickOverflow)?;
        self.pending_birth = Some(VillagerPendingBirth {
            partner_uuid,
            started_tick,
            ready_tick,
        });
        Ok(ready_tick)
    }

    pub fn abort_pending_birth(&mut self) -> bool {
        self.pending_birth.take().is_some()
    }

    pub fn finish_courtship_without_child(
        &mut self,
        current_tick: u64,
        food_items: VillagerFoodItemIds,
    ) -> Result<(), VillagerPopulationError> {
        let Some(pending) = self.pending_birth.as_ref() else {
            return Err(VillagerPopulationError::MissingPendingBirth);
        };
        if current_tick < pending.ready_tick {
            return Err(VillagerPopulationError::PendingBirthMismatch);
        }
        self.inventory
            .eat_until_full(&mut self.food_level, food_items)?;
        self.food_level = self
            .food_level
            .checked_sub(VILLAGER_BREEDING_WILLINGNESS_POINTS)
            .ok_or(VillagerPopulationError::NotWilling)?;
        self.pending_birth = None;
        Ok(())
    }

    pub fn finish_successful_birth(
        &mut self,
        current_tick: u64,
        food_items: VillagerFoodItemIds,
    ) -> Result<(), VillagerPopulationError> {
        self.finish_courtship_without_child(current_tick, food_items)?;
        self.age_ticks = VILLAGER_PARENT_COOLDOWN_TICKS;
        Ok(())
    }

    /// Advances age toward zero and reports the one transition into adulthood.
    pub fn advance_age(&mut self, elapsed_ticks: u32) -> bool {
        if self.age_ticks == 0 || elapsed_ticks == 0 {
            return false;
        }
        let elapsed = i32::try_from(elapsed_ticks).unwrap_or(i32::MAX);
        let before = self.age_ticks;
        self.age_ticks = if before < 0 {
            before.saturating_add(elapsed).min(0)
        } else {
            before.saturating_sub(elapsed).max(0)
        };
        before < 0 && self.age_ticks == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VillagerPopulationError {
    FoodOverflow,
    InvalidStack,
    MissingPendingBirth,
    NotWilling,
    PendingBirthMismatch,
    TickOverflow,
}

#[must_use]
pub const fn villager_birth_delay_ticks(deterministic_seed: u64) -> u16 {
    VILLAGER_BIRTH_DELAY_MIN_TICKS
        + (deterministic_seed % VILLAGER_BIRTH_DELAY_RANDOM_BOUND as u64) as u16
}

#[must_use]
pub fn deterministic_villager_child_uuid(
    first_parent: Uuid,
    second_parent: Uuid,
    home_claim: &str,
    deterministic_seed: u64,
) -> Uuid {
    let (first, second) = if first_parent.as_u128() <= second_parent.as_u128() {
        (first_parent, second_parent)
    } else {
        (second_parent, first_parent)
    };
    let mut high = 0xCBF2_9CE4_8422_2325_u64;
    let mut low = 0x8422_2325_CBF2_9CE4_u64;
    for byte in first
        .as_bytes()
        .iter()
        .chain(second.as_bytes())
        .chain(home_claim.as_bytes())
        .chain(deterministic_seed.to_be_bytes().iter())
    {
        high = (high ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
        low = (low ^ u64::from(*byte))
            .rotate_left(5)
            .wrapping_mul(0x9E37_79B1_85EB_CA87);
    }
    let mut bytes = ((u128::from(high) << 64) | u128::from(low)).to_be_bytes();
    bytes[6] = (bytes[6] & 0x0F) | 0x50;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOD: VillagerFoodItemIds = VillagerFoodItemIds {
        bread: 1,
        potato: 2,
        carrot: 3,
        beetroot: 4,
    };

    #[test]
    fn mixed_inventory_food_uses_exact_vanilla_values_and_threshold() {
        let mut state = VillagerPopulationState::adult();
        state
            .add_to_inventory(EntityItemStack::new(FOOD.bread, 2), 64)
            .unwrap();
        state
            .add_to_inventory(EntityItemStack::new(FOOD.carrot, 3), 64)
            .unwrap();
        assert_eq!(state.inventory.food_points(FOOD), 11);
        assert!(!state.can_breed(false, FOOD));
        state
            .add_to_inventory(EntityItemStack::new(FOOD.beetroot, 1), 64)
            .unwrap();
        assert_eq!(state.total_food_points(FOOD), 12);
        assert!(state.can_breed(false, FOOD));
        assert!(!state.can_breed(true, FOOD));
    }

    #[test]
    fn eight_slot_inventory_merges_exact_components_and_returns_remainder() {
        let mut inventory = VillagerInventory::default();
        let named = EntityItemStack::new(10, 60).with_custom_name("named");
        assert!(inventory.add_stack(named.clone(), 64).unwrap().is_none());
        assert!(
            inventory
                .add_stack(EntityItemStack::new(10, 4).with_custom_name("named"), 64)
                .unwrap()
                .is_none()
        );
        assert_eq!(inventory.slots()[0].as_ref().unwrap().count, 64);
        assert!(
            inventory
                .add_stack(EntityItemStack::new(10, 64), 64)
                .unwrap()
                .is_none()
        );
        for item in 20..26 {
            assert!(
                inventory
                    .add_stack(EntityItemStack::new(item, 64), 64)
                    .unwrap()
                    .is_none()
            );
        }
        let remainder = inventory
            .add_stack(EntityItemStack::new(99, 70), 64)
            .unwrap()
            .expect("ninth-slot remainder");
        assert_eq!(remainder.count, 70);
        assert_eq!(
            inventory
                .slots()
                .iter()
                .filter(|slot| slot.is_some())
                .count(),
            8
        );
    }

    #[test]
    fn eating_uses_slot_order_and_preserves_bread_overshoot() {
        let mut state = VillagerPopulationState::adult();
        state.food_level = 11;
        state
            .add_to_inventory(EntityItemStack::new(FOOD.bread, 2), 64)
            .unwrap();
        state
            .start_pending_birth(Uuid::from_u128(2), 10, 0, false, FOOD)
            .unwrap();
        state.finish_courtship_without_child(285, FOOD).unwrap();
        assert_eq!(state.food_level, 3);
        assert_eq!(state.inventory.count_item(FOOD.bread), 1);
        assert_eq!(state.age_ticks, 0);
        assert!(state.pending_birth.is_none());
    }

    #[test]
    fn no_bed_digests_food_but_success_also_sets_parent_cooldown() {
        let mut no_bed = VillagerPopulationState::adult();
        no_bed
            .add_to_inventory(EntityItemStack::new(FOOD.bread, 3), 64)
            .unwrap();
        let ready = no_bed
            .start_pending_birth(Uuid::from_u128(2), 10, 49, false, FOOD)
            .unwrap();
        no_bed.finish_courtship_without_child(ready, FOOD).unwrap();
        assert_eq!(no_bed.food_level, 0);
        assert_eq!(no_bed.age_ticks, 0);

        let mut success = VillagerPopulationState::adult();
        success
            .add_to_inventory(EntityItemStack::new(FOOD.potato, 12), 64)
            .unwrap();
        success
            .start_pending_birth(Uuid::from_u128(3), 10, 0, false, FOOD)
            .unwrap();
        success.finish_successful_birth(285, FOOD).unwrap();
        assert_eq!(success.food_level, 0);
        assert_eq!(success.age_ticks, VILLAGER_PARENT_COOLDOWN_TICKS);
    }

    #[test]
    fn food_sharing_matches_vanilla_half_or_leave_twenty_four_rules() {
        for (count, expected_shared, expected_left) in [
            (24, 0, 24),
            (25, 1, 24),
            (32, 8, 24),
            (33, 16, 17),
            (64, 32, 32),
        ] {
            let mut inventory = VillagerInventory::default();
            inventory
                .add_stack(EntityItemStack::new(FOOD.carrot, count), 64)
                .unwrap();
            let shared = inventory.extract_food_share(FOOD, 64);
            assert_eq!(
                shared.as_ref().map_or(0, |stack| stack.count),
                expected_shared
            );
            assert_eq!(inventory.count_item(FOOD.carrot), expected_left);
        }
    }

    #[test]
    fn abort_preserves_inventory_and_food_level() {
        let mut state = VillagerPopulationState::adult();
        state.food_level = 4;
        state
            .add_to_inventory(EntityItemStack::new(FOOD.bread, 2), 64)
            .unwrap();
        state
            .start_pending_birth(Uuid::from_u128(2), 10, 1, false, FOOD)
            .unwrap();
        assert!(state.abort_pending_birth());
        assert_eq!(state.food_level, 4);
        assert_eq!(state.inventory.count_item(FOOD.bread), 2);
        assert!(!state.abort_pending_birth());
    }

    #[test]
    fn deterministic_birth_delay_spans_vanilla_inclusive_bounds() {
        assert_eq!(villager_birth_delay_ticks(0), 275);
        assert_eq!(villager_birth_delay_ticks(49), 324);
        assert_eq!(villager_birth_delay_ticks(50), 275);
    }

    #[test]
    fn baby_matures_once_at_exact_zero() {
        let mut child = VillagerPopulationState::baby("home:one".to_owned());
        assert!(!child.advance_age(23_999));
        assert_eq!(child.age_ticks, -1);
        assert!(child.advance_age(1));
        assert_eq!(child.age_ticks, 0);
        assert!(!child.advance_age(1));
    }

    #[test]
    fn child_uuid_is_parent_order_independent_and_seed_scoped() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let expected = deterministic_villager_child_uuid(first, second, "home:one", 7);
        assert_eq!(
            expected,
            deterministic_villager_child_uuid(second, first, "home:one", 7)
        );
        assert_ne!(
            expected,
            deterministic_villager_child_uuid(first, second, "home:one", 8)
        );
        assert_ne!(
            expected,
            deterministic_villager_child_uuid(first, second, "home:two", 7)
        );
    }
}
