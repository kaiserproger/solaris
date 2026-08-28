//! Protocol-neutral player survival resource transitions for Java Edition 26.1.2.

pub const MAX_HEALTH: f32 = 20.0;
pub const MAX_FOOD: i32 = 20;
pub const BLOCK_BREAK_EXHAUSTION: f32 = 0.005;
pub const ENTITY_ATTACK_EXHAUSTION: f32 = 0.1;
pub const SPRINT_EXHAUSTION_PER_METER: f32 = 0.1;
pub const JUMP_EXHAUSTION: f32 = 0.05;
pub const SPRINT_JUMP_EXHAUSTION: f32 = 0.2;
pub const EXHAUSTION_STEP: f32 = 4.0;
const SATURATED_REGEN_TICKS: u32 = 10;
const HEALTH_TICK_PERIOD: u32 = 80;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurvivalResources {
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub exhaustion: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurvivalHealthTick {
    Unchanged,
    Changed,
    StarvationDamage(f32),
}

#[must_use]
pub fn apply_damage(health: f32, amount: f32) -> f32 {
    (health - amount.max(0.0)).clamp(0.0, MAX_HEALTH)
}

#[must_use]
pub fn heal(health: f32, amount: f32) -> f32 {
    (health + amount.max(0.0)).clamp(0.0, MAX_HEALTH)
}

#[must_use]
pub fn is_dead(health: f32) -> bool {
    health <= 0.0
}

#[must_use]
pub fn can_eat(health: f32, food: i32) -> bool {
    !is_dead(health) && food < MAX_FOOD
}

#[must_use]
pub fn add_food(food: i32, saturation: f32, added_food: i32, added_saturation: f32) -> (i32, f32) {
    let food = food.saturating_add(added_food).clamp(0, MAX_FOOD);
    let saturation = if saturation.is_finite() {
        saturation.max(0.0)
    } else {
        0.0
    };
    let added_saturation = if added_saturation.is_finite() {
        added_saturation.max(0.0)
    } else {
        0.0
    };
    (food, (saturation + added_saturation).min(food as f32))
}

#[must_use]
pub fn add_exhaustion(resources: SurvivalResources, amount: f32) -> SurvivalResources {
    let mut food = resources.food.clamp(0, MAX_FOOD);
    let mut saturation = if resources.saturation.is_finite() {
        resources.saturation.clamp(0.0, food as f32)
    } else {
        0.0
    };
    let current_exhaustion = if resources.exhaustion.is_finite() {
        resources.exhaustion.max(0.0)
    } else {
        0.0
    };
    let added_exhaustion = if amount.is_finite() {
        amount.max(0.0)
    } else {
        0.0
    };
    let total = current_exhaustion + added_exhaustion;
    let total = if total.is_finite() { total } else { f32::MAX };
    let resource_steps = (MAX_FOOD * 2) as u32;
    let steps = ((total / EXHAUSTION_STEP).floor() as u32).min(resource_steps);
    let exhaustion = total % EXHAUSTION_STEP;

    let saturation_steps = steps.min(saturation.ceil() as u32);
    saturation = (saturation - saturation_steps as f32).max(0.0);
    let food_steps = steps - saturation_steps;
    food = food.saturating_sub(food_steps as i32).max(0);

    SurvivalResources {
        food,
        saturation,
        exhaustion,
        ..resources
    }
}

#[must_use]
pub fn tick_health(
    mut resources: SurvivalResources,
    tick_timer: &mut u32,
) -> (SurvivalResources, SurvivalHealthTick) {
    if resources.health <= 0.0 {
        *tick_timer = 0;
        return (resources, SurvivalHealthTick::Unchanged);
    }

    if resources.saturation > 0.0 && resources.food >= MAX_FOOD && resources.health < MAX_HEALTH {
        *tick_timer = tick_timer.saturating_add(1);
        if *tick_timer < SATURATED_REGEN_TICKS {
            return (resources, SurvivalHealthTick::Unchanged);
        }
        *tick_timer = 0;
        let saturation_spent = resources.saturation.min(6.0);
        resources.health = heal(resources.health, saturation_spent / 6.0);
        resources = add_exhaustion(resources, saturation_spent);
        return (resources, SurvivalHealthTick::Changed);
    }

    if resources.food >= 18 && resources.health < MAX_HEALTH {
        *tick_timer = tick_timer.saturating_add(1);
        if *tick_timer < HEALTH_TICK_PERIOD {
            return (resources, SurvivalHealthTick::Unchanged);
        }
        *tick_timer = 0;
        resources.health = heal(resources.health, 1.0);
        resources = add_exhaustion(resources, 6.0);
        return (resources, SurvivalHealthTick::Changed);
    }

    if resources.food == 0 {
        *tick_timer = tick_timer.saturating_add(1);
        if *tick_timer < HEALTH_TICK_PERIOD {
            return (resources, SurvivalHealthTick::Unchanged);
        }
        *tick_timer = 0;
        return (resources, SurvivalHealthTick::StarvationDamage(1.0));
    }

    *tick_timer = 0;
    (resources, SurvivalHealthTick::Unchanged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> SurvivalResources {
        SurvivalResources {
            health: MAX_HEALTH,
            food: MAX_FOOD,
            saturation: 5.0,
            exhaustion: 0.0,
        }
    }

    #[test]
    fn eating_requires_a_live_non_full_player() {
        assert!(can_eat(MAX_HEALTH, MAX_FOOD - 1));
        assert!(!can_eat(MAX_HEALTH, MAX_FOOD));
        assert!(!can_eat(0.0, MAX_FOOD - 1));
    }

    #[test]
    fn exhaustion_spends_saturation_before_food() {
        let state = add_exhaustion(
            SurvivalResources {
                saturation: 2.0,
                ..full()
            },
            12.0,
        );
        assert_eq!(state.saturation, 0.0);
        assert_eq!(state.food, 19);
        assert!((0.0..EXHAUSTION_STEP).contains(&state.exhaustion));
    }

    #[test]
    fn health_tick_covers_saturated_regen_and_starvation() {
        let mut timer = 9;
        let (saturated, outcome) = tick_health(
            SurvivalResources {
                health: 19.0,
                ..full()
            },
            &mut timer,
        );
        assert_eq!(outcome, SurvivalHealthTick::Changed);
        assert!(saturated.health > 19.0);

        let mut timer = 79;
        let (_, outcome) = tick_health(
            SurvivalResources {
                food: 0,
                saturation: 0.0,
                ..full()
            },
            &mut timer,
        );
        assert_eq!(outcome, SurvivalHealthTick::StarvationDamage(1.0));
    }
}
