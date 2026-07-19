use std::collections::HashMap;

use mc_entity::{
    ANIMAL_BREEDING_COURTSHIP_TICKS, ANIMAL_LOVE_DURATION_TICKS, AnimalBreedingState, EntityId,
    PARENT_BREEDING_COOLDOWN_TICKS, SheepColor, Vec3,
};
use mc_world::BlockPos;

mod authority;

pub(super) const SHEEP_GRAZING_ANIMATION_TICKS: u8 = 40;
pub(in crate::play) const SHEEP_GRAZING_ACTION_TICK: u8 = 4;

#[derive(Debug, Clone)]
pub(super) struct BreedingAnimal {
    pub(super) id: EntityId,
    pub(super) type_id: i32,
    pub(super) type_name: String,
    pub(super) position: Vec3,
    pub(super) state: AnimalBreedingState,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct BreedingBirth {
    pub(super) type_id: i32,
    pub(super) type_name: String,
    pub(super) position: Vec3,
    pub(super) sheep_color: Option<SheepColor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BreedingStateUpdate {
    pub(super) entity_id: EntityId,
    pub(super) state: AnimalBreedingState,
}

#[derive(Debug, Default)]
pub(super) struct BreedingPlan {
    pub(super) updates: Vec<BreedingStateUpdate>,
    pub(super) births: Vec<BreedingBirth>,
    pub(super) became_adults: Vec<EntityId>,
}

pub(super) fn plan_breeding(simulation_tick: u64, animals: &[BreedingAnimal]) -> BreedingPlan {
    let mut updates = animals
        .iter()
        .map(|animal| BreedingStateUpdate {
            entity_id: animal.id,
            state: animal.state,
        })
        .collect::<Vec<_>>();
    let mut became_adults = Vec::new();
    for update in &mut updates {
        let was_baby = update.state.is_baby();
        if update.state.age_ticks < 0 {
            update.state.age_ticks += 1;
        } else if update.state.age_ticks > 0 {
            update.state.age_ticks -= 1;
        }
        if update.state.age_ticks == 0 {
            update.state.love_ticks = update.state.love_ticks.saturating_sub(1);
        } else {
            update.state.love_ticks = 0;
        }
        if was_baby && !update.state.is_baby() {
            became_adults.push(update.entity_id);
        }
    }

    let courtship_complete = ANIMAL_LOVE_DURATION_TICKS - ANIMAL_BREEDING_COURTSHIP_TICKS;
    let mut paired = vec![false; animals.len()];
    let mut births = Vec::new();
    for first_index in 0..animals.len() {
        let first = &animals[first_index];
        let first_state = updates[first_index].state;
        if paired[first_index]
            || first_state.age_ticks != 0
            || first_state.love_ticks == 0
            || first_state.love_ticks > courtship_complete
        {
            continue;
        }
        let Some(second_index) = ((first_index + 1)..animals.len()).find(|&second_index| {
            let second = &animals[second_index];
            let second_state = updates[second_index].state;
            !paired[second_index]
                && second.type_name == first.type_name
                && second_state.age_ticks == 0
                && second_state.love_ticks > 0
                && second_state.love_ticks <= courtship_complete
                && distance_sq(first.position, second.position) < 9.0
        }) else {
            continue;
        };

        let second = &animals[second_index];
        let sheep_color = if first.type_name == "minecraft:sheep" {
            updates[first_index]
                .state
                .sheep_wool
                .zip(updates[second_index].state.sheep_wool)
                .map(|(first_wool, second_wool)| {
                    sheep_breeding_color(
                        first.id,
                        first_wool.color,
                        second.id,
                        second_wool.color,
                        simulation_tick,
                    )
                })
        } else {
            None
        };
        paired[first_index] = true;
        paired[second_index] = true;
        births.push(BreedingBirth {
            type_id: first.type_id,
            type_name: first.type_name.clone(),
            position: Vec3::new(
                (first.position.x + second.position.x) * 0.5,
                (first.position.y + second.position.y) * 0.5,
                (first.position.z + second.position.z) * 0.5,
            ),
            sheep_color,
        });
        updates[first_index].state = AnimalBreedingState {
            age_ticks: PARENT_BREEDING_COOLDOWN_TICKS,
            love_ticks: 0,
            ..updates[first_index].state
        };
        updates[second_index].state = AnimalBreedingState {
            age_ticks: PARENT_BREEDING_COOLDOWN_TICKS,
            love_ticks: 0,
            ..updates[second_index].state
        };
    }

    BreedingPlan {
        updates,
        births,
        became_adults,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct GrazingSheep {
    pub(super) entity_id: EntityId,
    pub(super) position: Vec3,
    pub(super) is_baby: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) struct SheepGrazingCandidate {
    pub(in crate::play) entity_id: EntityId,
    pub(in crate::play) block_position: BlockPos,
}

#[derive(Debug, Default)]
pub(in crate::play) struct SheepGrazingPlan {
    pub(in crate::play) starts: Vec<SheepGrazingCandidate>,
    pub(in crate::play) actions: Vec<SheepGrazingCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SheepGrazingTimerUpdate {
    pub(super) entity_id: EntityId,
    pub(super) remaining: Option<u8>,
}

pub(super) struct SheepGrazingAdvance {
    pub(super) plan: SheepGrazingPlan,
    pub(super) timer_updates: Vec<SheepGrazingTimerUpdate>,
}

pub(super) fn advance_sheep_grazing(
    tick: u64,
    sheep: &[GrazingSheep],
    timers: &HashMap<EntityId, u8>,
) -> SheepGrazingAdvance {
    let mut plan = SheepGrazingPlan::default();
    let mut timer_updates = Vec::with_capacity(sheep.len().min(timers.len()));

    for sheep in sheep {
        let candidate = SheepGrazingCandidate {
            entity_id: sheep.entity_id,
            block_position: BlockPos {
                x: sheep.position.x.floor() as i32,
                y: sheep.position.y.floor() as i32,
                z: sheep.position.z.floor() as i32,
            },
        };
        let Some(remaining) = timers.get(&sheep.entity_id).copied() else {
            if sheep_grazing_starts_on_tick(sheep.entity_id, tick, sheep.is_baby) {
                plan.starts.push(candidate);
            }
            continue;
        };

        let remaining = remaining.saturating_sub(1);
        if remaining == 0 {
            timer_updates.push(SheepGrazingTimerUpdate {
                entity_id: sheep.entity_id,
                remaining: None,
            });
        } else {
            timer_updates.push(SheepGrazingTimerUpdate {
                entity_id: sheep.entity_id,
                remaining: Some(remaining),
            });
            if remaining == SHEEP_GRAZING_ACTION_TICK {
                plan.actions.push(candidate);
            }
        }
    }

    SheepGrazingAdvance {
        plan,
        timer_updates,
    }
}

pub(in crate::play) fn sheep_grazing_starts_on_tick(
    entity_id: EntityId,
    tick: u64,
    is_baby: bool,
) -> bool {
    let period = if is_baby { 50 } else { 1_000 };
    let mut phase = entity_id.0 as i64 as u64;
    phase = (phase ^ (phase >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    phase = (phase ^ (phase >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    phase ^= phase >> 31;
    tick % period == phase % period
}

pub(super) fn sheep_recipe_mix(first: SheepColor, second: SheepColor) -> Option<SheepColor> {
    match (first, second) {
        (SheepColor::Red, SheepColor::Yellow) | (SheepColor::Yellow, SheepColor::Red) => {
            Some(SheepColor::Orange)
        }
        (SheepColor::Purple, SheepColor::Pink) | (SheepColor::Pink, SheepColor::Purple) => {
            Some(SheepColor::Magenta)
        }
        (SheepColor::Blue, SheepColor::White) | (SheepColor::White, SheepColor::Blue) => {
            Some(SheepColor::LightBlue)
        }
        (SheepColor::Green, SheepColor::White) | (SheepColor::White, SheepColor::Green) => {
            Some(SheepColor::Lime)
        }
        (SheepColor::Red, SheepColor::White) | (SheepColor::White, SheepColor::Red) => {
            Some(SheepColor::Pink)
        }
        (SheepColor::Black, SheepColor::White) | (SheepColor::White, SheepColor::Black) => {
            Some(SheepColor::Gray)
        }
        (SheepColor::Gray, SheepColor::White) | (SheepColor::White, SheepColor::Gray) => {
            Some(SheepColor::LightGray)
        }
        (SheepColor::Blue, SheepColor::Green) | (SheepColor::Green, SheepColor::Blue) => {
            Some(SheepColor::Cyan)
        }
        (SheepColor::Blue, SheepColor::Red) | (SheepColor::Red, SheepColor::Blue) => {
            Some(SheepColor::Purple)
        }
        _ => None,
    }
}

pub(super) fn sheep_breeding_color(
    first_id: EntityId,
    first_color: SheepColor,
    second_id: EntityId,
    second_color: SheepColor,
    simulation_tick: u64,
) -> SheepColor {
    if let Some(mixed) = sheep_recipe_mix(first_color, second_color) {
        return mixed;
    }
    let (low_id, low_color, high_id, high_color) = if first_id.0 <= second_id.0 {
        (first_id, first_color, second_id, second_color)
    } else {
        (second_id, second_color, first_id, first_color)
    };
    let seed = (low_id.0 as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (high_id.0 as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ simulation_tick.wrapping_mul(0x1656_67B1_9E37_79F9);
    if seed.rotate_left(23) & 1 == 0 {
        low_color
    } else {
        high_color
    }
}

fn distance_sq(first: Vec3, second: Vec3) -> f64 {
    let dx = first.x - second.x;
    let dy = first.y - second.y;
    let dz = first.z - second.z;
    dx * dx + dy * dy + dz * dz
}
