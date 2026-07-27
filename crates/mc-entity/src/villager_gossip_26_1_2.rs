//! Persisted villager gossip state for Java Edition 26.1.2.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_PLAYER_GOSSIPS: usize = 64;
pub const MAX_TRANSFER_COUNT: usize = 10;
const DISCARD_THRESHOLD: i32 = 2;
const DAY_LENGTH_TICKS: i64 = 24_000;
const TRADING_MAX: i32 = 25;
const TRADING_ADD: i32 = 2;
const TRADING_DECAY_PER_DAY: i32 = 2;
const MINOR_NEGATIVE_MAX: i32 = 200;
const MINOR_NEGATIVE_ADD: i32 = 25;
const MINOR_NEGATIVE_DECAY_PER_DAY: i32 = 20;
const MAJOR_NEGATIVE_MAX: i32 = 100;
const MAJOR_NEGATIVE_ADD: i32 = 25;
const MAJOR_NEGATIVE_DECAY_PER_DAY: i32 = 10;
const MINOR_POSITIVE_MAX: i32 = 25;
const MINOR_POSITIVE_ADD: i32 = 25;
const MINOR_POSITIVE_DECAY_PER_DAY: i32 = 1;
const MAJOR_POSITIVE_MAX: i32 = 20;
const MAJOR_POSITIVE_ADD: i32 = 20;
const MAJOR_POSITIVE_DECAY_PER_DAY: i32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VillagerGossipEvent {
    Trade { player: Uuid },
    HurtByPlayer { player: Uuid },
    KilledByPlayer { player: Uuid },
    ZombieVillagerCured { player: Uuid },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VillagerPlayerGossip {
    pub player: Uuid,
    #[serde(default)]
    pub trading: i32,
    #[serde(default)]
    pub minor_negative: i32,
    #[serde(default)]
    pub major_negative: i32,
    pub minor_positive: i32,
    pub major_positive: i32,
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
                || !valid_stored_value(gossip.major_negative, MAJOR_NEGATIVE_MAX)
                || !valid_stored_value(gossip.minor_positive, MINOR_POSITIVE_MAX)
                || !valid_stored_value(gossip.major_positive, MAJOR_POSITIVE_MAX)
                || gossip.trading == 0
                    && gossip.minor_negative == 0
                    && gossip.major_negative == 0
                    && gossip.minor_positive == 0
                    && gossip.major_positive == 0
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
            .map_or(0, |gossip| {
                gossip.trading + gossip.minor_positive + 5 * gossip.major_positive
                    - gossip.minor_negative
                    - 5 * gossip.major_negative
            })
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

    #[must_use]
    pub fn major_negative_value(&self, player: Uuid) -> i32 {
        self.player_gossips
            .iter()
            .find(|gossip| gossip.player == player)
            .map_or(0, |gossip| gossip.major_negative)
    }

    #[must_use]
    pub fn minor_positive_value(&self, player: Uuid) -> i32 {
        self.player_gossips
            .iter()
            .find(|gossip| gossip.player == player)
            .map_or(0, |gossip| gossip.minor_positive)
    }

    #[must_use]
    pub fn major_positive_value(&self, player: Uuid) -> i32 {
        self.player_gossips
            .iter()
            .find(|gossip| gossip.player == player)
            .map_or(0, |gossip| gossip.major_positive)
    }

    pub fn record_event(&mut self, event: VillagerGossipEvent) -> bool {
        if let VillagerGossipEvent::ZombieVillagerCured { player } = event {
            let Some(gossip) = self.player_gossip_mut_or_insert(player) else {
                return false;
            };
            let next_minor = gossip
                .minor_positive
                .saturating_add(MINOR_POSITIVE_ADD)
                .min(MINOR_POSITIVE_MAX);
            let next_major = gossip
                .major_positive
                .saturating_add(MAJOR_POSITIVE_ADD)
                .min(MAJOR_POSITIVE_MAX);
            let changed =
                next_minor != gossip.minor_positive || next_major != gossip.major_positive;
            gossip.minor_positive = next_minor;
            gossip.major_positive = next_major;
            return changed;
        }

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
            VillagerGossipEvent::KilledByPlayer { player } => (
                player,
                GossipField::MajorNegative,
                MAJOR_NEGATIVE_ADD,
                MAJOR_NEGATIVE_MAX,
            ),
            VillagerGossipEvent::ZombieVillagerCured { .. } => unreachable!(),
        };
        let Some(gossip) = self.player_gossip_mut_or_insert(player) else {
            return false;
        };
        let value = field.value_mut(gossip);
        let next = value.saturating_add(amount).min(maximum);
        if next == *value {
            return false;
        }
        *value = next;
        true
    }

    /// Applies the Java 26.1.2 weighted-transfer shape with Java legacy
    /// `nextInt(bound)` draws. The caller owns the seed; reproducing the complete
    /// per-entity vanilla `RandomSource` stream is outside this state container.
    pub fn transfer_from_seeded(
        &mut self,
        source: &Self,
        seed: u64,
        max_count: usize,
    ) -> Result<bool, VillagerGossipError> {
        self.validate()?;
        source.validate()?;
        let entries = source.transfer_entries();
        if entries.is_empty() || max_count == 0 {
            return Ok(false);
        }
        let total_weight = entries
            .iter()
            .map(|entry| entry.selection_weight)
            .sum::<u32>();
        if total_weight == 0 {
            return Ok(false);
        }

        let mut random = JavaLegacyRandom::new(seed as i64);
        let mut selected =
            Vec::<(Uuid, GossipField)>::with_capacity(max_count.min(MAX_TRANSFER_COUNT));
        for _ in 0..max_count.min(MAX_TRANSFER_COUNT) {
            let choice = random.next_int(total_weight);
            let mut cumulative = 0_u32;
            let Some(entry) = entries.iter().find(|entry| {
                cumulative = cumulative.saturating_add(entry.selection_weight);
                choice < cumulative
            }) else {
                continue;
            };
            if !selected.contains(&(entry.player, entry.field)) {
                selected.push((entry.player, entry.field));
            }
        }

        let mut changed = false;
        for (player, field) in selected {
            let Some(entry) = entries
                .iter()
                .find(|entry| entry.player == player && entry.field == field)
            else {
                continue;
            };
            let transferred = entry.value.saturating_sub(field.transfer_decay());
            if transferred < DISCARD_THRESHOLD {
                continue;
            }
            changed |= self.merge_transferred(player, field, transferred);
        }
        self.validate()?;
        Ok(changed)
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
                    major_negative: 0,
                    minor_positive: 0,
                    major_positive: 0,
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
            gossip.major_negative =
                decayed_value(gossip.major_negative, MAJOR_NEGATIVE_DECAY_PER_DAY);
            gossip.minor_positive =
                decayed_value(gossip.minor_positive, MINOR_POSITIVE_DECAY_PER_DAY);
            gossip.major_positive =
                decayed_value(gossip.major_positive, MAJOR_POSITIVE_DECAY_PER_DAY);
        }
        self.player_gossips.retain(|gossip| {
            gossip.trading != 0
                || gossip.minor_negative != 0
                || gossip.major_negative != 0
                || gossip.minor_positive != 0
                || gossip.major_positive != 0
        });
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
            major_negative: 0,
            minor_positive: 0,
            major_positive: 0,
        });
        self.player_gossips.last_mut()
    }

    fn transfer_entries(&self) -> Vec<TransferEntry> {
        let mut entries = Vec::with_capacity(self.player_gossips.len().saturating_mul(5));
        for gossip in &self.player_gossips {
            if gossip.major_negative > 0 {
                entries.push(TransferEntry {
                    player: gossip.player,
                    field: GossipField::MajorNegative,
                    value: gossip.major_negative,
                    selection_weight: u32::try_from(gossip.major_negative)
                        .expect("validated gossip is positive")
                        .saturating_mul(5),
                });
            }
            if gossip.minor_negative > 0 {
                entries.push(TransferEntry {
                    player: gossip.player,
                    field: GossipField::MinorNegative,
                    value: gossip.minor_negative,
                    selection_weight: u32::try_from(gossip.minor_negative)
                        .expect("validated gossip is positive"),
                });
            }
            if gossip.minor_positive > 0 {
                entries.push(TransferEntry {
                    player: gossip.player,
                    field: GossipField::MinorPositive,
                    value: gossip.minor_positive,
                    selection_weight: u32::try_from(gossip.minor_positive)
                        .expect("validated gossip is positive"),
                });
            }
            if gossip.major_positive > 0 {
                entries.push(TransferEntry {
                    player: gossip.player,
                    field: GossipField::MajorPositive,
                    value: gossip.major_positive,
                    selection_weight: u32::try_from(gossip.major_positive)
                        .expect("validated gossip is positive")
                        .saturating_mul(5),
                });
            }
            if gossip.trading > 0 {
                entries.push(TransferEntry {
                    player: gossip.player,
                    field: GossipField::Trading,
                    value: gossip.trading,
                    selection_weight: u32::try_from(gossip.trading)
                        .expect("validated gossip is positive"),
                });
            }
        }
        entries
    }

    fn merge_transferred(&mut self, player: Uuid, field: GossipField, value: i32) -> bool {
        let Some(gossip) = self.player_gossip_mut_or_insert(player) else {
            return false;
        };
        let current = field.value_mut(gossip);
        if *current >= value {
            return false;
        }
        *current = value;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VillagerGossipError {
    InvalidGameTime,
    InvalidState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GossipField {
    Trading,
    MinorNegative,
    MajorNegative,
    MinorPositive,
    MajorPositive,
}

#[derive(Debug, Clone, Copy)]
struct TransferEntry {
    player: Uuid,
    field: GossipField,
    value: i32,
    selection_weight: u32,
}

impl GossipField {
    fn value_mut(self, gossip: &mut VillagerPlayerGossip) -> &mut i32 {
        match self {
            Self::Trading => &mut gossip.trading,
            Self::MinorNegative => &mut gossip.minor_negative,
            Self::MajorNegative => &mut gossip.major_negative,
            Self::MinorPositive => &mut gossip.minor_positive,
            Self::MajorPositive => &mut gossip.major_positive,
        }
    }

    const fn transfer_decay(self) -> i32 {
        match self {
            Self::Trading | Self::MinorNegative | Self::MajorPositive => 20,
            Self::MajorNegative => 10,
            Self::MinorPositive => 5,
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

const JAVA_RANDOM_MULTIPLIER: u64 = 0x5DEECE66D;
const JAVA_RANDOM_ADDEND: u64 = 0xB;
const JAVA_RANDOM_MASK: u64 = (1_u64 << 48) - 1;

struct JavaLegacyRandom {
    seed: u64,
}

impl JavaLegacyRandom {
    fn new(seed: i64) -> Self {
        Self {
            seed: ((seed as u64) ^ JAVA_RANDOM_MULTIPLIER) & JAVA_RANDOM_MASK,
        }
    }

    fn next_bits(&mut self, bits: u32) -> u32 {
        self.seed = self
            .seed
            .wrapping_mul(JAVA_RANDOM_MULTIPLIER)
            .wrapping_add(JAVA_RANDOM_ADDEND)
            & JAVA_RANDOM_MASK;
        (self.seed >> (48 - bits)) as u32
    }

    fn next_int(&mut self, bound: u32) -> u32 {
        debug_assert!(bound > 0 && bound <= i32::MAX as u32);
        if bound.is_power_of_two() {
            return ((u64::from(bound) * u64::from(self.next_bits(31))) >> 31) as u32;
        }
        loop {
            let bits = self.next_bits(31);
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) as i32 >= 0 {
                return value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_hurt_kill_and_cure_events_use_exact_26_1_2_values_weights_and_caps() {
        let player = Uuid::from_u128(7);
        let mut gossip = VillagerGossipState::default();

        assert!(gossip.record_event(VillagerGossipEvent::Trade { player }));
        assert_eq!(gossip.trading_value(player), 2);
        assert_eq!(gossip.player_reputation(player), 2);
        assert!(gossip.record_event(VillagerGossipEvent::HurtByPlayer { player }));
        assert_eq!(gossip.minor_negative_value(player), 25);
        assert_eq!(gossip.player_reputation(player), -23);
        assert!(gossip.record_event(VillagerGossipEvent::KilledByPlayer { player }));
        assert_eq!(gossip.major_negative_value(player), 25);
        assert_eq!(gossip.player_reputation(player), -148);
        assert!(gossip.record_event(VillagerGossipEvent::ZombieVillagerCured { player }));
        assert_eq!(gossip.minor_positive_value(player), 25);
        assert_eq!(gossip.major_positive_value(player), 20);
        assert_eq!(gossip.player_reputation(player), -23);

        for _ in 0..100 {
            gossip.record_event(VillagerGossipEvent::Trade { player });
            gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
            gossip.record_event(VillagerGossipEvent::KilledByPlayer { player });
            gossip.record_event(VillagerGossipEvent::ZombieVillagerCured { player });
        }
        assert_eq!(gossip.trading_value(player), TRADING_MAX);
        assert_eq!(gossip.minor_negative_value(player), MINOR_NEGATIVE_MAX);
        assert_eq!(gossip.major_negative_value(player), MAJOR_NEGATIVE_MAX);
        assert_eq!(gossip.minor_positive_value(player), MINOR_POSITIVE_MAX);
        assert_eq!(gossip.major_positive_value(player), MAJOR_POSITIVE_MAX);
        assert_eq!(gossip.player_reputation(player), -550);
        gossip.validate().unwrap();
    }

    #[test]
    fn daily_decay_uses_per_type_amounts_and_discards_values_below_two() {
        let player = Uuid::from_u128(7);
        let mut gossip = VillagerGossipState::default();
        gossip.record_event(VillagerGossipEvent::Trade { player });
        gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
        gossip.record_event(VillagerGossipEvent::KilledByPlayer { player });

        assert!(!gossip.decay(0).unwrap());
        assert_eq!(gossip.last_decay_game_time, 0);
        assert!(gossip.decay(100).unwrap());
        assert_eq!(gossip.last_decay_game_time, 100);
        assert!(!gossip.decay(24_099).unwrap());
        assert_eq!(gossip.trading_value(player), 2);
        assert_eq!(gossip.minor_negative_value(player), 25);
        assert_eq!(gossip.major_negative_value(player), 25);
        assert!(gossip.decay(24_100).unwrap());
        assert_eq!(gossip.trading_value(player), 0);
        assert_eq!(gossip.minor_negative_value(player), 5);
        assert_eq!(gossip.major_negative_value(player), 15);
        assert!(gossip.decay(48_100).unwrap());
        assert_eq!(gossip.minor_negative_value(player), 0);
        assert_eq!(gossip.major_negative_value(player), 5);
        assert_eq!(gossip.player_reputation(player), -25);
        assert!(gossip.decay(72_100).unwrap());
        assert_eq!(gossip.player_reputation(player), 0);
        assert!(gossip.player_gossips.is_empty());
    }

    #[test]
    fn cured_villager_positive_gossip_decays_minor_and_keeps_major_memory() {
        let player = Uuid::from_u128(8);
        let mut gossip = VillagerGossipState::default();

        assert!(gossip.record_event(VillagerGossipEvent::ZombieVillagerCured { player }));
        assert_eq!(gossip.minor_positive_value(player), 25);
        assert_eq!(gossip.major_positive_value(player), 20);
        assert_eq!(gossip.player_reputation(player), 125);

        assert!(gossip.decay(100).unwrap());
        for day in 1..=24 {
            assert!(gossip.decay(100 + day * DAY_LENGTH_TICKS).unwrap());
        }
        assert_eq!(gossip.minor_positive_value(player), 0);
        assert_eq!(gossip.major_positive_value(player), 20);
        assert_eq!(gossip.player_reputation(player), 100);
        gossip.validate().unwrap();
    }

    #[test]
    fn java_legacy_next_int_matches_known_seeded_draw() {
        let mut random = JavaLegacyRandom::new(0x1234_ABCD);
        assert_eq!(random.next_int(20), 3);
    }

    #[test]
    fn weighted_transfer_applies_type_decay_deduplicates_draws_and_merges_by_max() {
        let player = Uuid::from_u128(7);
        let mut source = VillagerGossipState::default();
        for _ in 0..13 {
            source.record_event(VillagerGossipEvent::Trade { player });
        }
        for _ in 0..2 {
            source.record_event(VillagerGossipEvent::HurtByPlayer { player });
        }
        assert_eq!(source.trading_value(player), 25);
        source.record_event(VillagerGossipEvent::KilledByPlayer { player });
        source.record_event(VillagerGossipEvent::KilledByPlayer { player });
        assert_eq!(source.minor_negative_value(player), 50);
        assert_eq!(source.major_negative_value(player), 50);

        let mut receiver = VillagerGossipState::default();
        assert!(
            receiver
                .transfer_from_seeded(&source, 0xA11C_E5E5, MAX_TRANSFER_COUNT)
                .unwrap()
        );
        assert!(matches!(receiver.trading_value(player), 0 | 5));
        assert!(matches!(receiver.minor_negative_value(player), 0 | 30));
        assert_eq!(receiver.major_negative_value(player), 40);
        assert!(
            receiver.trading_value(player) != 0
                || receiver.minor_negative_value(player) != 0
                || receiver.major_negative_value(player) != 0
        );

        let once = receiver.clone();
        receiver
            .transfer_from_seeded(&source, 0xA11C_E5E5, MAX_TRANSFER_COUNT)
            .unwrap();
        assert_eq!(receiver, once, "transfer merge must use max, not addition");
    }

    #[test]
    fn transfer_drops_values_below_the_storage_floor_and_never_exceeds_ten_unique_entries() {
        let mut source = VillagerGossipState::default();
        for index in 0..20_u128 {
            let player = Uuid::from_u128(index + 1);
            source.player_gossips.push(VillagerPlayerGossip {
                player,
                trading: if index == 0 { 21 } else { 25 },
                minor_negative: 0,
                major_negative: 0,
                minor_positive: 0,
                major_positive: 0,
            });
        }
        source.validate().unwrap();

        let mut receiver = VillagerGossipState::default();
        receiver
            .transfer_from_seeded(&source, 0x55AA_1234, MAX_TRANSFER_COUNT)
            .unwrap();
        assert_eq!(receiver.trading_value(Uuid::from_u128(1)), 0);
        assert!(receiver.player_gossips.len() <= MAX_TRANSFER_COUNT);
        assert!(receiver.player_gossips.iter().all(|entry| {
            entry.trading == 5
                && entry.minor_negative == 0
                && entry.major_negative == 0
                && entry.minor_positive == 0
                && entry.major_positive == 0
        }));
    }

    #[test]
    fn positive_transfer_uses_exact_type_decay() {
        let player = Uuid::from_u128(9);
        let source = VillagerGossipState {
            last_decay_game_time: 0,
            player_gossips: vec![VillagerPlayerGossip {
                player,
                trading: 0,
                minor_negative: 0,
                major_negative: 0,
                minor_positive: 25,
                major_positive: 0,
            }],
        };
        let mut receiver = VillagerGossipState::default();

        assert!(receiver.transfer_from_seeded(&source, 7, 1).unwrap());
        assert_eq!(receiver.minor_positive_value(player), 20);
        assert_eq!(receiver.major_positive_value(player), 0);

        let everlasting_source = VillagerGossipState {
            last_decay_game_time: 0,
            player_gossips: vec![VillagerPlayerGossip {
                player,
                trading: 0,
                minor_negative: 0,
                major_negative: 0,
                minor_positive: 0,
                major_positive: 20,
            }],
        };
        let mut other_receiver = VillagerGossipState::default();
        assert!(
            !other_receiver
                .transfer_from_seeded(&everlasting_source, 7, 1)
                .unwrap()
        );
        assert_eq!(other_receiver.player_reputation(player), 0);
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
                    major_negative: 0,
                    minor_positive: 0,
                    major_positive: 0,
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
