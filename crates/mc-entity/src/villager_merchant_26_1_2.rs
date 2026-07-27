//! Persisted, protocol-neutral villager merchant state for Java Edition 26.1.2.

use serde::{Deserialize, Serialize};

use crate::EntityItemStack;

const MAX_OFFERS: usize = 32;
const MAX_STACK_COUNT: i32 = 64;
const MAX_RESTOCKS_PER_DAY: u8 = 2;
const DAY_LENGTH_TICKS: i64 = 24_000;
const SECOND_RESTOCK_DELAY_TICKS: i64 = 2_400;
const RESTOCK_DAY_RESET_TICKS: i64 = 12_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VillagerTradeCost {
    pub item_id: u32,
    pub count: i32,
}

impl VillagerTradeCost {
    #[must_use]
    pub const fn new(item_id: u32, count: i32) -> Self {
        Self { item_id, count }
    }

    fn validate(self) -> Result<(), VillagerMerchantError> {
        if !(1..=MAX_STACK_COUNT).contains(&self.count) {
            return Err(VillagerMerchantError::InvalidStackCount(self.count));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VillagerTradeOffer {
    pub cost_a: VillagerTradeCost,
    pub cost_b: Option<VillagerTradeCost>,
    pub result: EntityItemStack,
    pub uses: i32,
    pub max_uses: i32,
    pub reward_exp: bool,
    pub xp: i32,
    pub special_price: i32,
    pub demand: i32,
    pub price_multiplier: f32,
}

impl VillagerTradeOffer {
    #[must_use]
    pub fn new(
        cost_a: VillagerTradeCost,
        result: EntityItemStack,
        max_uses: i32,
        xp: i32,
        price_multiplier: f32,
    ) -> Self {
        Self {
            cost_a,
            cost_b: None,
            result,
            uses: 0,
            max_uses,
            reward_exp: true,
            xp,
            special_price: 0,
            demand: 0,
            price_multiplier,
        }
    }

    pub fn validate(&self) -> Result<(), VillagerMerchantError> {
        self.cost_a.validate()?;
        if let Some(cost_b) = self.cost_b {
            cost_b.validate()?;
        }
        if !(1..=MAX_STACK_COUNT).contains(&self.result.count) {
            return Err(VillagerMerchantError::InvalidStackCount(self.result.count));
        }
        if self.uses < 0 || self.max_uses <= 0 || self.uses > self.max_uses || self.xp < 0 {
            return Err(VillagerMerchantError::InvalidCounters);
        }
        if !self.price_multiplier.is_finite() || self.price_multiplier < 0.0 {
            return Err(VillagerMerchantError::InvalidPriceMultiplier);
        }
        Ok(())
    }

    #[must_use]
    pub fn modified_cost_a_count(&self, item_max_stack: i32) -> i32 {
        let item_max_stack = item_max_stack.clamp(1, MAX_STACK_COUNT);
        let demand_delta = ((self.cost_a.count as f32 * self.demand as f32) * self.price_multiplier)
            .floor()
            .max(0.0) as i32;
        (self.cost_a.count + demand_delta + self.special_price).clamp(1, item_max_stack)
    }

    #[must_use]
    pub fn is_out_of_stock(&self) -> bool {
        self.uses >= self.max_uses
    }

    fn record_use(&mut self) -> Result<(), VillagerMerchantError> {
        if self.is_out_of_stock() {
            return Err(VillagerMerchantError::OutOfStock);
        }
        self.uses += 1;
        Ok(())
    }

    fn restock(&mut self) {
        self.demand = self.demand + self.uses - (self.max_uses - self.uses);
        self.uses = 0;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VillagerMerchantState {
    pub offers: Vec<VillagerTradeOffer>,
    pub xp: i32,
    pub restocks_today: u8,
    pub last_restock_day: i64,
    #[serde(default)]
    pub last_restock_game_time: Option<i64>,
}

impl VillagerMerchantState {
    pub fn new(offers: Vec<VillagerTradeOffer>) -> Result<Self, VillagerMerchantError> {
        let state = Self {
            offers,
            xp: 0,
            restocks_today: 0,
            last_restock_day: i64::MIN,
            last_restock_game_time: None,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn validate(&self) -> Result<(), VillagerMerchantError> {
        if self.offers.len() > MAX_OFFERS || self.xp < 0 {
            return Err(VillagerMerchantError::InvalidCounters);
        }
        if self.restocks_today > MAX_RESTOCKS_PER_DAY
            || self.last_restock_game_time.is_some_and(|time| time < 0)
        {
            return Err(VillagerMerchantError::InvalidCounters);
        }
        for offer in &self.offers {
            offer.validate()?;
        }
        Ok(())
    }

    pub fn record_trade(
        &mut self,
        offer_index: usize,
    ) -> Result<(EntityItemStack, i32), VillagerMerchantError> {
        let offer = self
            .offers
            .get_mut(offer_index)
            .ok_or(VillagerMerchantError::UnknownOffer)?;
        offer.record_use()?;
        self.xp = self.xp.saturating_add(offer.xp);
        Ok((offer.result.clone(), offer.xp))
    }

    pub fn restock(&mut self, game_time: i64) -> Result<bool, VillagerMerchantError> {
        if game_time < 0 {
            return Err(VillagerMerchantError::InvalidGameTime);
        }
        let day = game_time.div_euclid(DAY_LENGTH_TICKS);
        if day != self.last_restock_day {
            self.last_restock_day = day;
            self.restocks_today = 0;
        }
        if self.restocks_today >= MAX_RESTOCKS_PER_DAY
            || !self.offers.iter().any(|offer| offer.uses > 0)
            || self
                .last_restock_game_time
                .is_some_and(|last| game_time.saturating_sub(last) < RESTOCK_COOLDOWN_TICKS)
        {
            return Ok(false);
        }
        for offer in &mut self.offers {
            if offer.uses > 0 {
                offer.restock();
            }
        }
        self.restocks_today += 1;
        self.last_restock_game_time = Some(game_time);
        self.validate()?;
        Ok(true)
    }

    #[must_use]
    pub fn level(&self) -> u8 {
        match self.xp {
            0..=9 => 1,
            10..=69 => 2,
            70..=149 => 3,
            150..=249 => 4,
            _ => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VillagerMerchantError {
    InvalidStackCount(i32),
    InvalidCounters,
    InvalidPriceMultiplier,
    InvalidGameTime,
    UnknownOffer,
    OutOfStock,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> VillagerTradeOffer {
        VillagerTradeOffer::new(
            VillagerTradeCost::new(1, 15),
            EntityItemStack::new(2, 1),
            16,
            2,
            0.05,
        )
    }

    #[test]
    fn demand_special_price_and_stack_limit_match_vanilla_order() {
        let mut offer = offer();
        offer.demand = 4;
        offer.special_price = -2;
        assert_eq!(offer.modified_cost_a_count(64), 16);
        offer.special_price = -100;
        assert_eq!(offer.modified_cost_a_count(64), 1);
        offer.special_price = 100;
        assert_eq!(offer.modified_cost_a_count(16), 16);
    }

    #[test]
    fn trade_use_xp_levels_and_out_of_stock_are_persisted_state() {
        let mut merchant = VillagerMerchantState::new(vec![offer()]).unwrap();
        for _ in 0..5 {
            let (result, xp) = merchant.record_trade(0).unwrap();
            assert_eq!(result, EntityItemStack::new(2, 1));
            assert_eq!(xp, 2);
        }
        assert_eq!(merchant.xp, 10);
        assert_eq!(merchant.level(), 2);
        merchant.offers[0].uses = merchant.offers[0].max_uses;
        assert_eq!(
            merchant.record_trade(0),
            Err(VillagerMerchantError::OutOfStock)
        );
    }

    #[test]
    fn restock_updates_demand_and_enforces_cooldown_and_daily_limit() {
        let mut merchant = VillagerMerchantState::new(vec![offer()]).unwrap();
        merchant.offers[0].uses = 3;
        assert!(merchant.restock(4_000).unwrap());
        assert_eq!(merchant.offers[0].uses, 0);
        assert_eq!(merchant.offers[0].demand, -10);

        merchant.offers[0].uses = 1;
        assert!(!merchant.restock(4_200).unwrap());
        assert_eq!(merchant.offers[0].uses, 1);
        assert!(merchant.restock(5_200).unwrap());

        merchant.offers[0].uses = 1;
        assert!(!merchant.restock(6_400).unwrap());
        assert_eq!(merchant.offers[0].uses, 1);
        assert!(merchant.restock(28_000).unwrap());
        assert_eq!(merchant.restocks_today, 1);
        assert_eq!(merchant.last_restock_day, 1);
    }

    #[test]
    fn restock_rejects_negative_time_without_mutating_state() {
        let mut merchant = VillagerMerchantState::new(vec![offer()]).unwrap();
        merchant.offers[0].uses = 1;
        let expected = merchant.clone();
        assert_eq!(
            merchant.restock(-1),
            Err(VillagerMerchantError::InvalidGameTime)
        );
        assert_eq!(merchant, expected);
    }
}
