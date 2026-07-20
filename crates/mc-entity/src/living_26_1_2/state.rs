pub const HURT_DURATION_TICKS: u32 = 10;
pub const INVULNERABLE_DURATION_TICKS: u32 = 20;
pub const DEATH_DURATION_TICKS: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivingLifecycle {
    Alive,
    Dying,
    Removed,
}

/// Scalar state copied into and out of an ECS component row by the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivingState {
    pub health: f32,
    pub absorption: f32,
    pub invulnerable_time: u32,
    pub hurt_time: u32,
    pub last_hurt: f32,
    pub lifecycle: LivingLifecycle,
    pub death_time: u32,
}

impl LivingState {
    pub fn new(health: f32, absorption: f32) -> Result<Self, StateError> {
        let state = Self {
            health,
            absorption,
            invulnerable_time: 0,
            hurt_time: 0,
            last_hurt: 0.0,
            lifecycle: LivingLifecycle::Alive,
            death_time: 0,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn validate(self) -> Result<(), StateError> {
        if !self.health.is_finite() {
            return Err(StateError::NonFiniteHealth);
        }
        if !self.absorption.is_finite() {
            return Err(StateError::NonFiniteAbsorption);
        }
        if !self.last_hurt.is_finite() {
            return Err(StateError::NonFiniteLastHurt);
        }
        if self.health < 0.0 {
            return Err(StateError::NegativeHealth);
        }
        if self.absorption < 0.0 {
            return Err(StateError::NegativeAbsorption);
        }

        match self.lifecycle {
            LivingLifecycle::Alive if self.health <= 0.0 => Err(StateError::AliveWithoutHealth),
            LivingLifecycle::Alive if self.death_time != 0 => Err(StateError::AliveWithDeathTime),
            LivingLifecycle::Dying if self.health > 0.0 => Err(StateError::DyingWithHealth),
            LivingLifecycle::Dying if self.death_time >= DEATH_DURATION_TICKS => {
                Err(StateError::DyingPastRemoval)
            }
            LivingLifecycle::Removed if self.health > 0.0 => Err(StateError::RemovedWithHealth),
            LivingLifecycle::Removed if self.death_time < DEATH_DURATION_TICKS => {
                Err(StateError::RemovedBeforeDeathTime)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    NonFiniteHealth,
    NonFiniteAbsorption,
    NonFiniteLastHurt,
    NegativeHealth,
    NegativeAbsorption,
    AliveWithoutHealth,
    AliveWithDeathTime,
    DyingWithHealth,
    DyingPastRemoval,
    RemovedWithHealth,
    RemovedBeforeDeathTime,
}

/// Vanilla decrements this field in `LivingEntity.tick` for non-player living
/// entities. `ServerPlayer` owns the same clock elsewhere, so its adapter must
/// select `External`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvulnerabilityClock {
    Kernel,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickApplied {
    Stable,
    RemovedNow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    Advanced(TickApplied),
    InvalidState(StateError),
}

pub fn tick_living(
    state: &mut LivingState,
    invulnerability_clock: InvulnerabilityClock,
    output: &mut TickOutcome,
) {
    if let Err(error) = state.validate() {
        *output = TickOutcome::InvalidState(error);
        return;
    }

    let mut next = *state;
    if next.lifecycle != LivingLifecycle::Removed {
        next.hurt_time = next.hurt_time.saturating_sub(1);
        if invulnerability_clock == InvulnerabilityClock::Kernel {
            next.invulnerable_time = next.invulnerable_time.saturating_sub(1);
        }
    }

    let tick_result = if next.lifecycle == LivingLifecycle::Dying {
        next.death_time += 1;
        if next.death_time >= DEATH_DURATION_TICKS {
            next.lifecycle = LivingLifecycle::Removed;
            TickApplied::RemovedNow
        } else {
            TickApplied::Stable
        }
    } else {
        TickApplied::Stable
    };

    *state = next;
    *output = TickOutcome::Advanced(tick_result);
}
