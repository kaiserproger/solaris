const SAFE_TICK_PERCENT: u64 = 85;
const RECOVERY_HEADROOM_PERCENT: u64 = 75;
const SIMULATION_QUEUE_PRESSURE_DEPTH: usize = 512;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EntityUpdatePressure {
    pub(crate) reliable_drops_increased: bool,
    pub(crate) reliable_retries_in_flight: u64,
    pub(crate) simulation_queue_depth: usize,
}

impl EntityUpdatePressure {
    fn is_active(self) -> bool {
        self.reliable_drops_increased
            || self.reliable_retries_in_flight > 0
            || self.simulation_queue_depth >= SIMULATION_QUEUE_PRESSURE_DEPTH
    }
}

const INITIAL_MOVEMENT_PUBLICATION_UPDATES: usize = 512;
const MIN_MOVEMENT_PUBLICATION_UPDATES: usize = 512;
const MAX_MOVEMENT_PUBLICATION_UPDATES: usize = 2_048;

#[derive(Debug, Clone, Copy)]
pub(crate) struct MovementPublicationBudgetController {
    configured: usize,
}

impl Default for MovementPublicationBudgetController {
    fn default() -> Self {
        Self {
            configured: INITIAL_MOVEMENT_PUBLICATION_UPDATES,
        }
    }
}

impl MovementPublicationBudgetController {
    pub(crate) fn observe(
        &mut self,
        tick_us: u64,
        target_tick_us: u64,
        pressure: EntityUpdatePressure,
    ) -> usize {
        let safe_tick_us = target_tick_us
            .saturating_mul(SAFE_TICK_PERCENT)
            .div_ceil(100);
        let recovery_tick_us = safe_tick_us
            .saturating_mul(RECOVERY_HEADROOM_PERCENT)
            .div_ceil(100);
        self.configured = if pressure.reliable_drops_increased {
            self.configured.div_ceil(2)
        } else if pressure.is_active() || tick_us > safe_tick_us {
            self.configured.saturating_mul(3).div_ceil(4)
        } else if tick_us < recovery_tick_us {
            self.configured.saturating_add((self.configured / 8).max(1))
        } else {
            self.configured
        }
        .clamp(
            MIN_MOVEMENT_PUBLICATION_UPDATES,
            MAX_MOVEMENT_PUBLICATION_UPDATES,
        );
        self.configured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_publication_budget_grows_only_with_headroom() {
        let mut controller = MovementPublicationBudgetController::default();
        let grown = controller.observe(20_000, 50_000, EntityUpdatePressure::default());
        assert!(grown > INITIAL_MOVEMENT_PUBLICATION_UPDATES);

        let held = controller.observe(40_000, 50_000, EntityUpdatePressure::default());
        assert_eq!(held, grown);
    }

    #[test]
    fn movement_publication_pressure_returns_to_safe_floor() {
        let mut controller = MovementPublicationBudgetController { configured: 2_048 };
        let reduced = controller.observe(
            30_000,
            50_000,
            EntityUpdatePressure {
                reliable_drops_increased: true,
                ..EntityUpdatePressure::default()
            },
        );
        assert_eq!(reduced, 1_024);
        let floor = controller.observe(
            30_000,
            50_000,
            EntityUpdatePressure {
                reliable_drops_increased: true,
                ..EntityUpdatePressure::default()
            },
        );
        assert_eq!(floor, MIN_MOVEMENT_PUBLICATION_UPDATES);
    }
}
