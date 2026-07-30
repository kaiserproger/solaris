const INITIAL_UPDATES_PER_LANE: usize = 1_024;
const MIN_UPDATES_PER_LANE: usize = 64;
const MAX_UPDATES_PER_LANE: usize = 8_192;
const MAX_ROTATION_TICKS: usize = 40;
const ENTITY_TICK_SHARE_PERCENT: u64 = 60;
const SAFE_TICK_PERCENT: u64 = 85;
const RECOVERY_HEADROOM_PERCENT: u64 = 75;
const SIMULATION_QUEUE_PRESSURE_DEPTH: usize = 512;
const COST_SCALE: u64 = 1 << 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EntityUpdatePressure {
    pub(crate) reliable_drops_increased: bool,
    pub(crate) reliable_retries_in_flight: u64,
    pub(crate) simulation_queue_depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntityUpdateBudgetObservation {
    pub(crate) tick_us: u64,
    pub(crate) entity_goals_us: u64,
    pub(crate) selected: usize,
    pub(crate) active_population: usize,
    pub(crate) lane_count: usize,
    pub(crate) target_tick_us: u64,
    pub(crate) pressure: EntityUpdatePressure,
}

impl EntityUpdatePressure {
    fn is_active(self) -> bool {
        self.reliable_drops_increased
            || self.reliable_retries_in_flight > 0
            || self.simulation_queue_depth >= SIMULATION_QUEUE_PRESSURE_DEPTH
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntityUpdateBudgetSnapshot {
    pub(crate) configured_per_lane: usize,
    pub(crate) effective_total: usize,
    pub(crate) selected: usize,
    pub(crate) active_population: usize,
    pub(crate) estimated_rotation_ticks: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityUpdateBudgetController {
    configured_per_lane: usize,
    smoothed_cost_per_update_scaled: u64,
}

impl Default for EntityUpdateBudgetController {
    fn default() -> Self {
        Self {
            configured_per_lane: INITIAL_UPDATES_PER_LANE,
            smoothed_cost_per_update_scaled: 0,
        }
    }
}

impl EntityUpdateBudgetController {
    pub(crate) fn configured_per_lane(&self) -> usize {
        self.configured_per_lane
    }

    pub(crate) fn observe(
        &mut self,
        observation: EntityUpdateBudgetObservation,
    ) -> EntityUpdateBudgetSnapshot {
        let EntityUpdateBudgetObservation {
            tick_us,
            entity_goals_us,
            selected,
            active_population,
            lane_count,
            target_tick_us,
            pressure,
        } = observation;
        let lanes = lane_count.max(1);
        let freshness_total = active_population.div_ceil(MAX_ROTATION_TICKS);
        let freshness_per_lane = freshness_total.div_ceil(lanes);

        if selected > 0 && entity_goals_us > 0 {
            let sample = entity_goals_us
                .saturating_mul(COST_SCALE)
                .checked_div(selected as u64)
                .unwrap_or(u64::MAX)
                .max(1);
            self.smoothed_cost_per_update_scaled = if self.smoothed_cost_per_update_scaled == 0 {
                sample
            } else {
                self.smoothed_cost_per_update_scaled
                    .saturating_mul(3)
                    .saturating_add(sample)
                    / 4
            };

            let safe_tick_us = target_tick_us
                .saturating_mul(SAFE_TICK_PERCENT)
                .div_ceil(100);
            let non_entity_us = tick_us.saturating_sub(entity_goals_us);
            let entity_share_cap_us = target_tick_us
                .saturating_mul(ENTITY_TICK_SHARE_PERCENT)
                .div_ceil(100);
            let entity_target_us = safe_tick_us
                .saturating_sub(non_entity_us)
                .max(1)
                .min(entity_share_cap_us);
            let estimated_total = entity_target_us
                .saturating_mul(COST_SCALE)
                .checked_div(self.smoothed_cost_per_update_scaled)
                .unwrap_or(usize::MAX as u64)
                .clamp(1, usize::MAX as u64) as usize;
            let desired_per_lane = estimated_total
                .max(freshness_total)
                .div_ceil(lanes)
                .clamp(MIN_UPDATES_PER_LANE, MAX_UPDATES_PER_LANE);

            self.configured_per_lane = if pressure.reliable_drops_increased {
                self.configured_per_lane
                    .div_ceil(2)
                    .min(desired_per_lane)
                    .max(freshness_per_lane)
            } else if pressure.is_active() || tick_us > safe_tick_us {
                self.configured_per_lane
                    .saturating_mul(3)
                    .div_ceil(4)
                    .min(desired_per_lane)
                    .max(freshness_per_lane)
            } else if tick_us.saturating_mul(100)
                < safe_tick_us.saturating_mul(RECOVERY_HEADROOM_PERCENT)
            {
                let growth = (self.configured_per_lane / 8).max(1);
                self.configured_per_lane
                    .saturating_add(growth)
                    .min(desired_per_lane.max(self.configured_per_lane))
            } else if desired_per_lane > self.configured_per_lane {
                self.configured_per_lane
            } else {
                self.configured_per_lane
                    .saturating_sub((self.configured_per_lane - desired_per_lane).div_ceil(8))
                    .max(freshness_per_lane)
            };
            self.configured_per_lane = self
                .configured_per_lane
                .clamp(MIN_UPDATES_PER_LANE, MAX_UPDATES_PER_LANE);
        } else {
            self.configured_per_lane = self
                .configured_per_lane
                .max(freshness_per_lane)
                .clamp(MIN_UPDATES_PER_LANE, MAX_UPDATES_PER_LANE);
        }

        let effective_total = self
            .configured_per_lane
            .saturating_mul(lanes)
            .max(freshness_total)
            .min(active_population.max(1));
        EntityUpdateBudgetSnapshot {
            configured_per_lane: self.configured_per_lane,
            effective_total,
            selected,
            active_population,
            estimated_rotation_ticks: active_population.div_ceil(effective_total.max(1)),
        }
    }
}

pub(crate) fn freshness_budget(active_population: usize) -> usize {
    active_population.div_ceil(MAX_ROTATION_TICKS)
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
    fn forty_thousand_entities_never_rotate_slower_than_two_seconds() {
        let mut controller = EntityUpdateBudgetController::default();
        let snapshot = controller.observe(EntityUpdateBudgetObservation {
            tick_us: 330_000,
            entity_goals_us: 325_000,
            selected: 512,
            active_population: 40_000,
            lane_count: 2,
            target_tick_us: 50_000,
            pressure: EntityUpdatePressure::default(),
        });

        assert!(snapshot.effective_total >= 1_000);
        assert!(snapshot.estimated_rotation_ticks <= MAX_ROTATION_TICKS);
    }

    #[test]
    fn healthy_ticks_recover_entity_throughput() {
        let mut controller = EntityUpdateBudgetController {
            configured_per_lane: 128,
            smoothed_cost_per_update_scaled: 0,
        };
        let before = controller.configured_per_lane();
        let snapshot = controller.observe(EntityUpdateBudgetObservation {
            tick_us: 10_000,
            entity_goals_us: 5_000,
            selected: 256,
            active_population: 4_000,
            lane_count: 4,
            target_tick_us: 50_000,
            pressure: EntityUpdatePressure::default(),
        });

        assert!(snapshot.configured_per_lane > before);
    }

    #[test]
    fn overloaded_ticks_reduce_budget_without_breaking_freshness_floor() {
        let mut controller = EntityUpdateBudgetController::default();
        let snapshot = controller.observe(EntityUpdateBudgetObservation {
            tick_us: 100_000,
            entity_goals_us: 90_000,
            selected: 2_048,
            active_population: 40_000,
            lane_count: 2,
            target_tick_us: 50_000,
            pressure: EntityUpdatePressure::default(),
        });

        assert!(snapshot.configured_per_lane <= INITIAL_UPDATES_PER_LANE);
        assert!(snapshot.effective_total >= freshness_budget(40_000));
    }

    #[test]
    fn reliable_drop_pressure_halves_budget_but_keeps_freshness() {
        let mut controller = EntityUpdateBudgetController::default();
        let snapshot = controller.observe(EntityUpdateBudgetObservation {
            tick_us: 35_000,
            entity_goals_us: 15_000,
            selected: 2_048,
            active_population: 40_000,
            lane_count: 2,
            target_tick_us: 50_000,
            pressure: EntityUpdatePressure {
                reliable_drops_increased: true,
                ..EntityUpdatePressure::default()
            },
        });

        assert!(snapshot.effective_total >= freshness_budget(40_000));
        assert!(snapshot.effective_total <= freshness_budget(40_000) + 24);
        assert_eq!(snapshot.estimated_rotation_ticks, MAX_ROTATION_TICKS);
    }

    #[test]
    fn non_entity_work_reduces_entity_allowance_before_target_tick_is_exceeded() {
        let mut controller = EntityUpdateBudgetController::default();
        let snapshot = controller.observe(EntityUpdateBudgetObservation {
            tick_us: 44_000,
            entity_goals_us: 20_000,
            selected: 2_048,
            active_population: 40_000,
            lane_count: 2,
            target_tick_us: 50_000,
            pressure: EntityUpdatePressure::default(),
        });

        assert!(snapshot.configured_per_lane <= INITIAL_UPDATES_PER_LANE);
        assert!(snapshot.effective_total >= freshness_budget(40_000));
    }

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
