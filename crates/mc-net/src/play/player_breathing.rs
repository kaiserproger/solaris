use mc_protocol::packets::play::{ENTITY_DATA_AIR_SUPPLY_INDEX, EntityDataValue, GameMode};

pub(super) const PLAYER_AIR_SUPPLY_METADATA_INDEX: u8 = ENTITY_DATA_AIR_SUPPLY_INDEX;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlayerBreathingTick {
    pub(super) air_changed: bool,
    pub(super) drowning_damage: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PlayerBreathingState {
    air_supply: i32,
}

impl Default for PlayerBreathingState {
    fn default() -> Self {
        Self {
            air_supply: Self::MAX_AIR_SUPPLY,
        }
    }
}

impl PlayerBreathingState {
    pub(super) const MAX_AIR_SUPPLY: i32 = 300;
    const DROWNING_DAMAGE_AIR: i32 = -20;
    const AIR_RECOVERY_PER_TICK: i32 = 4;
    const DROWNING_DAMAGE: f32 = 2.0;

    pub(super) fn tick(self, eye_in_water: bool, can_drown: bool) -> (Self, PlayerBreathingTick) {
        let mut next = self;
        let mut drowning_damage = 0.0;
        if eye_in_water && can_drown {
            next.air_supply = next.air_supply.saturating_sub(1);
            if next.air_supply <= Self::DROWNING_DAMAGE_AIR {
                next.air_supply = 0;
                drowning_damage = Self::DROWNING_DAMAGE;
            }
        } else if next.air_supply < Self::MAX_AIR_SUPPLY {
            next.air_supply = next
                .air_supply
                .saturating_add(Self::AIR_RECOVERY_PER_TICK)
                .min(Self::MAX_AIR_SUPPLY);
        }
        (
            next,
            PlayerBreathingTick {
                air_changed: next.air_supply != self.air_supply,
                drowning_damage,
            },
        )
    }

    pub(super) fn metadata(self) -> EntityDataValue {
        EntityDataValue::Int {
            index: PLAYER_AIR_SUPPLY_METADATA_INDEX,
            value: self.air_supply,
        }
    }

    pub(super) fn reset(&mut self) -> bool {
        let changed = self.air_supply != Self::MAX_AIR_SUPPLY;
        self.air_supply = Self::MAX_AIR_SUPPLY;
        changed
    }

    #[cfg(test)]
    pub(super) fn air_supply(self) -> i32 {
        self.air_supply
    }
}

pub(super) fn player_can_drown(game_mode: GameMode, is_dead: bool) -> bool {
    !is_dead && matches!(game_mode, GameMode::Survival | GameMode::Adventure)
}
