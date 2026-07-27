//! Persisted villager gossip state for Java Edition 26.1.2.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_PLAYER_GOSSIPS: usize = 64;
const DISCARD_THRESHOLD: i32 = 2;
const DAY_LENGTH_TICKS: i64 = 24_000;
const TRADING_MAX: i32 = 25;
const TRADING_ADD: i32 = 2;
const TRADING_DECAY_PER_DAY: i32 = 2;
const MINOR_NEGATIVE_MAX: i32 = 200;
const MINOR_NEGATIVE_ADD: i32 = 25;
const MINOR_NEGATIVE_DECAY_PER_DAY: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VillagerGossipEvent {
    Trade { player: Uuid },
    HurtByPlayer { player: Uuid },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VillagerPlayerGossip {
    pub player: Uuid,
    #[serde(default)]
    pub trading: i32,
    #[serde(default)]
    pub minor_negative: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VillagerGossipState {
    #[serde(default)]
    pub last_decay_game_time: i64,
    #[serde(default)]
    pub player_gossips: Vec<VillagerPlayerGossip>,
}

impl VillagerGossipState {
    pub fn validate(&self) -> Result<(), VillagerGossipError> {
        if self.player_gossips.len() > MAX_PLAYER_GOSSIPS || self.last_decay_game_time < 0 {
            return Err(VillagerGossipError::InvalidState);
        }
        for (index, gossip) in self.player_gossips.iter().enumerate() {
            if !valid_stored_value(gossip.trading, TRADING_MAX)
                || !valid_stored_value(gossip.minor_negative, MINOR_NEGATIVE_MAX)
                || gossip.trading == 0 && gossip.minor_negative == 0
                || self.player_gossips[..index]
                    .iter()
                    .any(|existing| existing.player == gossip.player)
            {
                return Err(VillagerGossipError::InvalidState);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn player_reputation(&self, player: Uuid) -> i32 {
        self.player_gossips
            .iter()
            .find(|gossip| gossip.player == player)
            .map_or(0, |gossip| gossip.trading - gossip.minor_negative)
    }

    #[must_use]
    pub fn trading_value(&self, player: Uuid) -> i32 {
        self.player_gossips
            .iter()
            .find(|gossip| gossip.player == player)
            .map_or(0, |gossip| gossip.trading)
    }

    #[must_use]
    pub fn minor_negative_value(&self, player: Uuid) -> i32 {
        self.player_gossips
            .iter()
            .find(|gossip| gossip.player == player)
            .map_or(0, |gossip| gossip.minor_negative)
    }

    pub fn record_event(&mut self, event: VillagerGossipEvent) -> bool {
        let (player, field, amount, maximum) = match event {
            VillagerGossipEvent::Trade { player } => {
                (player, GossipField::Trading, TRADING_ADD, TRADING_MAX)
            }
            VillagerGossipEvent::HurtByPlayer { player } => (
                player,
                GossipField::MinorNegative,
                MINOR_NEGATIVE_ADD,
                MINOR_NEGATIVE_MAX,
            ),
        };
        let Some(gossip) = self.player_gossip_mut_or_insert(player) else {
            return false;
        };
        let value = field.value_mut(gossip);
        *value = value.saturating_add(amount).min(maximum);
        true
    }

    pub fn merge_legacy_trading(
        &mut self,
        last_decay_game_time: Option<i64>,
        entries: impl IntoIterator<Item = (Uuid, i32)>,
    ) -> Result<(), VillagerGossipError> {
        let mut next = self.clone();
        if let Some(last_decay_game_time) = last_decay_game_time {
            if last_decay_game_time < 0 {
                return Err(VillagerGossipError::InvalidState);
            }
            next.last_decay_game_time = next.last_decay_game_time.max(last_decay_game_time);
        }
        for (player, trading) in entries {
            if !valid_stored_value(trading, TRADING_MAX) || trading == 0 {
                return Err(VillagerGossipError::InvalidState);
            }
            if let Some(existing) = next
                .player_gossips
                .iter_mut()
                .find(|gossip| gossip.player == player)
            {
                existing.trading = existing.trading.max(trading);
            } else {
                if next.player_gossips.len() >= MAX_PLAYER_GOSSIPS {
                    return Err(VillagerGossipError::InvalidState);
                }
                next.player_gossips.push(VillagerPlayerGossip {
                    player,
                    trading,
                    minor_negative: 0,
                });
            }
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn decay(&mut self, game_time: i64) -> Result<bool, VillagerGossipError> {
        if game_time < 0 {
            return Err(VillagerGossipError::InvalidGameTime);
        }
        if self.last_decay_game_time == 0 {
            if game_time == 0 {
                return Ok(false);
            }
            self.last_decay_game_time = game_time;
            return Ok(true);
        }
        if game_time < self.last_decay_game_time.saturating_add(DAY_LENGTH_TICKS) {
            return Ok(false);
        }
        for gossip in &mut self.player_gossips {
            gossip.trading = decayed_value(gossip.trading, TRADING_DECAY_PER_DAY);
            gossip.minor_negative =
                decayed_value(gossip.minor_negative, MINOR_NEGATIVE_DECAY_PER_DAY);
        }
        self.player_gossips
            .retain(|gossip| gossip.trading != 0 || gossip.minor_negative != 0);
        self.last_decay_game_time = game_time;
        self.validate()?;
        Ok(true)
    }

    fn player_gossip_mut_or_insert(&mut self, player: Uuid) -> Option<&mut VillagerPlayerGossip> {
        if let Some(index) = self
            .player_gossips
            .iter()
            .position(|gossip| gossip.player == player)
        {
            return self.player_gossips.get_mut(index);
        }
        if self.player_gossips.len() >= MAX_PLAYER_GOSSIPS {
            return None;
        }
        self.player_gossips.push(VillagerPlayerGossip {
            player,
            trading: 0,
            minor_negative: 0,
        });
        self.player_gossips.last_mut()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VillagerGossipError {
    InvalidGameTime,
    InvalidState,
}

#[derive(Debug, Clone, Copy)]
enum GossipField {
    Trading,
    MinorNegative,
}

impl GossipField {
    fn value_mut(self, gossip: &mut VillagerPlayerGossip) -> &mut i32 {
        match self {
            Self::Trading => &mut gossip.trading,
            Self::MinorNegative => &mut gossip.minor_negative,
        }
    }
}

fn valid_stored_value(value: i32, maximum: i32) -> bool {
    value == 0 || (DISCARD_THRESHOLD..=maximum).contains(&value)
}

fn decayed_value(value: i32, decay: i32) -> i32 {
    let value = value.saturating_sub(decay);
    if value < DISCARD_THRESHOLD { 0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_and_hurt_events_use_exact_26_1_2_values_weights_and_caps() {
        let player = Uuid::from_u128(7);
        let mut gossip = VillagerGossipState::default();

        assert!(gossip.record_event(VillagerGossipEvent::Trade { player }));
        assert_eq!(gossip.trading_value(player), 2);
        assert_eq!(gossip.player_reputation(player), 2);
        assert!(gossip.record_event(VillagerGossipEvent::HurtByPlayer { player }));
        assert_eq!(gossip.minor_negative_value(player), 25);
        assert_eq!(gossip.player_reputation(player), -23);

        for _ in 0..100 {
            gossip.record_event(VillagerGossipEvent::Trade { player });
            gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
        }
        assert_eq!(gossip.trading_value(player), TRADING_MAX);
        assert_eq!(gossip.minor_negative_value(player), MINOR_NEGATIVE_MAX);
        gossip.validate().unwrap();
    }

    #[test]
    fn daily_decay_uses_per_type_amounts_and_discards_values_below_two() {
        let player = Uuid::from_u128(7);
        let mut gossip = VillagerGossipState::default();
        gossip.record_event(VillagerGossipEvent::Trade { player });
        gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });

        assert!(!gossip.decay(0).unwrap());
        assert_eq!(gossip.last_decay_game_time, 0);
        assert!(gossip.decay(100).unwrap());
        assert_eq!(gossip.last_decay_game_time, 100);
        assert!(!gossip.decay(24_099).unwrap());
        assert_eq!(gossip.trading_value(player), 2);
        assert_eq!(gossip.minor_negative_value(player), 25);
        assert!(gossip.decay(24_100).unwrap());
        assert_eq!(gossip.trading_value(player), 0);
        assert_eq!(gossip.minor_negative_value(player), 5);
        assert!(gossip.decay(48_100).unwrap());
        assert_eq!(gossip.player_reputation(player), 0);
        assert!(gossip.player_gossips.is_empty());
    }

    #[test]
    fn full_ledger_drops_only_new_gossip_without_rejecting_the_event_source() {
        let mut gossip = VillagerGossipState {
            last_decay_game_time: 0,
            player_gossips: (0..MAX_PLAYER_GOSSIPS)
                .map(|index| VillagerPlayerGossip {
                    player: Uuid::from_u128(index as u128 + 1),
                    trading: 2,
                    minor_negative: 0,
                })
                .collect(),
        };
        let newcomer = Uuid::from_u128(1_000);

        assert!(!gossip.record_event(VillagerGossipEvent::HurtByPlayer { player: newcomer }));
        assert_eq!(gossip.player_reputation(newcomer), 0);
        assert_eq!(gossip.player_gossips.len(), MAX_PLAYER_GOSSIPS);
        gossip.validate().unwrap();
    }
}
