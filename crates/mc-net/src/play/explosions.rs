use super::inventory::PlayerInventory;
use super::session::VisibilityDispatch;
use super::{BlockEditBatchOutcome, BlockEditPrecondition};
use mc_entity::Vec3;
use mc_protocol::packets::play::{
    ClientboundExplode, EntityVec3, ExplosionBlockParticle, GameMode, ItemStack,
};
use mc_world::{BlockPos, BlockStateId};
use std::collections::HashSet;

// PrimedTnt.DEFAULT_FUSE_TIME in the bundled 26.1.2 server.
pub(super) const TNT_FUSE_TICKS: u64 = 80;
pub(super) const TNT_ENTITY_TYPE_NAME: &str = "minecraft:tnt";

pub(super) fn tnt_explosion_packet(
    position: Vec3,
    block_count: i32,
    knockback: Option<Vec3>,
) -> ClientboundExplode {
    ClientboundExplode {
        center: EntityVec3 {
            x: position.x,
            y: position.y + 0.06125,
            z: position.z,
        },
        radius: 4.0,
        block_count,
        knockback: knockback.map(|knockback| EntityVec3 {
            x: knockback.x,
            y: knockback.y,
            z: knockback.z,
        }),
        explosion_particle_id: 22,
        sound_reference_id: 697,
        block_particles: vec![
            ExplosionBlockParticle {
                particle_id: 59,
                scaling: 0.5,
                speed: 1.0,
                weight: 1,
            },
            ExplosionBlockParticle {
                particle_id: 62,
                scaling: 1.0,
                speed: 1.0,
                weight: 1,
            },
        ],
    }
}

const JAVA_RANDOM_MULTIPLIER: u64 = 0x5DEECE66D;
const JAVA_RANDOM_ADDEND: u64 = 0xB;
const JAVA_RANDOM_MASK: u64 = (1_u64 << 48) - 1;
const MAX_EXPLOSION_RADIUS: f32 = 64.0;
const MAX_RAY_STEPS: usize = 370;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct JavaLegacyRandom {
    seed: u64,
}

impl JavaLegacyRandom {
    pub(super) fn new(seed: i64) -> Self {
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

    pub(super) fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 / (1_u32 << 24) as f32
    }

    pub(super) fn next_double(&mut self) -> f64 {
        let high = u64::from(self.next_bits(26));
        let low = u64::from(self.next_bits(27));
        ((high << 27) | low) as f64 / (1_u64 << 53) as f64
    }

    pub(super) fn next_int(&mut self, bound: u32) -> u32 {
        assert!(bound > 0 && bound <= i32::MAX as u32);
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

    pub(super) fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let selected = self.next_int((index + 1) as u32) as usize;
            values.swap(index, selected);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ExplosionBlockSample {
    pub(super) resistance: Option<f32>,
    pub(super) explodable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExplosionPlannerError {
    InvalidCenter,
    InvalidRadius,
    InvalidResistance,
    RayStepLimitExceeded,
}

pub(super) fn plan_explosion_candidates(
    center: Vec3,
    radius: f32,
    random: &mut JavaLegacyRandom,
    mut sample_block: impl FnMut(BlockPos) -> Option<ExplosionBlockSample>,
) -> Result<HashSet<BlockPos>, ExplosionPlannerError> {
    if !center.x.is_finite() || !center.y.is_finite() || !center.z.is_finite() {
        return Err(ExplosionPlannerError::InvalidCenter);
    }
    if !radius.is_finite() || radius <= 0.0 || radius > MAX_EXPLOSION_RADIUS {
        return Err(ExplosionPlannerError::InvalidRadius);
    }

    let mut candidates = HashSet::new();
    for grid_x in 0..16 {
        for grid_y in 0..16 {
            for grid_z in 0..16 {
                if grid_x != 0
                    && grid_x != 15
                    && grid_y != 0
                    && grid_y != 15
                    && grid_z != 0
                    && grid_z != 15
                {
                    continue;
                }

                let mut direction_x = (grid_x as f32 / 15.0_f32 * 2.0_f32 - 1.0_f32) as f64;
                let mut direction_y = (grid_y as f32 / 15.0_f32 * 2.0_f32 - 1.0_f32) as f64;
                let mut direction_z = (grid_z as f32 / 15.0_f32 * 2.0_f32 - 1.0_f32) as f64;
                let direction_length = (direction_x * direction_x
                    + direction_y * direction_y
                    + direction_z * direction_z)
                    .sqrt();
                direction_x /= direction_length;
                direction_y /= direction_length;
                direction_z /= direction_length;

                let random_power = random.next_float() * 0.6_f32;
                let mut remaining_power = radius * (0.7_f32 + random_power);
                let mut x = center.x;
                let mut y = center.y;
                let mut z = center.z;
                let mut steps = 0;

                while remaining_power > 0.0 {
                    if steps == MAX_RAY_STEPS {
                        return Err(ExplosionPlannerError::RayStepLimitExceeded);
                    }
                    steps += 1;

                    let position = BlockPos {
                        x: x.floor() as i32,
                        y: y.floor() as i32,
                        z: z.floor() as i32,
                    };
                    let Some(sample) = sample_block(position) else {
                        break;
                    };

                    if let Some(resistance) = sample.resistance {
                        if !resistance.is_finite() || resistance < 0.0 {
                            return Err(ExplosionPlannerError::InvalidResistance);
                        }
                        remaining_power -= (resistance + 0.3_f32) * 0.3_f32;
                    }
                    if remaining_power > 0.0 && sample.explodable {
                        candidates.insert(position);
                    }

                    x += direction_x * f64::from(0.3_f32);
                    y += direction_y * f64::from(0.3_f32);
                    z += direction_z * f64::from(0.3_f32);
                    remaining_power -= 0.22500001_f32;
                }
            }
        }
    }

    Ok(candidates)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EntityExplosionImpact {
    pub(super) exposure: f32,
    pub(super) damage: f32,
    pub(super) knockback: Vec3,
}

pub(super) type PlayerExplosionImpact = EntityExplosionImpact;

pub(super) fn plan_player_explosion_impact(
    center: Vec3,
    radius: f32,
    player_feet: Vec3,
    collision_boxes: impl FnMut(BlockPos) -> Option<Vec<[f64; 6]>>,
) -> Option<PlayerExplosionImpact> {
    let eye = Vec3::new(player_feet.x, player_feet.y + 1.62, player_feet.z);
    let min = Vec3::new(player_feet.x - 0.3, player_feet.y, player_feet.z - 0.3);
    let max = Vec3::new(
        player_feet.x + 0.3,
        player_feet.y + 1.8,
        player_feet.z + 0.3,
    );
    plan_entity_explosion_impact(center, radius, player_feet, eye, min, max, collision_boxes)
}

pub(super) fn plan_entity_explosion_impact(
    center: Vec3,
    radius: f32,
    entity_position: Vec3,
    eye_position: Vec3,
    aabb_min: Vec3,
    aabb_max: Vec3,
    mut collision_boxes: impl FnMut(BlockPos) -> Option<Vec<[f64; 6]>>,
) -> Option<EntityExplosionImpact> {
    let double_radius = f64::from(radius) * 2.0;
    if double_radius <= 0.0
        || !double_radius.is_finite()
        || !center.x.is_finite()
        || !center.y.is_finite()
        || !center.z.is_finite()
        || !entity_position.x.is_finite()
        || !entity_position.y.is_finite()
        || !entity_position.z.is_finite()
        || !eye_position.x.is_finite()
        || !eye_position.y.is_finite()
        || !eye_position.z.is_finite()
        || !aabb_min.x.is_finite()
        || !aabb_min.y.is_finite()
        || !aabb_min.z.is_finite()
        || !aabb_max.x.is_finite()
        || !aabb_max.y.is_finite()
        || !aabb_max.z.is_finite()
        || aabb_min.x > aabb_max.x
        || aabb_min.y > aabb_max.y
        || aabb_min.z > aabb_max.z
    {
        return None;
    }

    let position_delta = Vec3::new(
        entity_position.x - center.x,
        entity_position.y - center.y,
        entity_position.z - center.z,
    );
    let position_distance = (position_delta.x * position_delta.x
        + position_delta.y * position_delta.y
        + position_delta.z * position_delta.z)
        .sqrt();
    let distance = position_distance / double_radius;
    if distance > 1.0 {
        return None;
    }

    let exposure = explosion_seen_percent(aabb_min, aabb_max, center, &mut collision_boxes);
    let power = (1.0 - distance) * f64::from(exposure);
    let damage = (((power * power + power) / 2.0) * 7.0 * double_radius + 1.0) as f32;

    let eye_delta = Vec3::new(
        eye_position.x - center.x,
        eye_position.y - center.y,
        eye_position.z - center.z,
    );
    let eye_distance =
        (eye_delta.x * eye_delta.x + eye_delta.y * eye_delta.y + eye_delta.z * eye_delta.z).sqrt();
    let direction = if eye_distance > 0.0 {
        Vec3::new(
            eye_delta.x / eye_distance,
            eye_delta.y / eye_distance,
            eye_delta.z / eye_distance,
        )
    } else {
        Vec3::ZERO
    };
    let knockback = Vec3::new(
        direction.x * power,
        direction.y * power,
        direction.z * power,
    );

    Some(EntityExplosionImpact {
        exposure,
        damage,
        knockback,
    })
}

fn explosion_seen_percent(
    min: Vec3,
    max: Vec3,
    center: Vec3,
    collision_boxes: &mut impl FnMut(BlockPos) -> Option<Vec<[f64; 6]>>,
) -> f32 {
    let x_step = 1.0 / ((max.x - min.x) * 2.0 + 1.0);
    let y_step = 1.0 / ((max.y - min.y) * 2.0 + 1.0);
    let z_step = 1.0 / ((max.z - min.z) * 2.0 + 1.0);
    let x_offset = (1.0 - (1.0 / x_step).floor() * x_step) / 2.0;
    let z_offset = (1.0 - (1.0 / z_step).floor() * z_step) / 2.0;
    let mut clear = 0_u32;
    let mut total = 0_u32;

    let mut x_fraction = 0.0;
    while x_fraction <= 1.0 {
        let mut y_fraction = 0.0;
        while y_fraction <= 1.0 {
            let mut z_fraction = 0.0;
            while z_fraction <= 1.0 {
                let from = Vec3::new(
                    min.x + (max.x - min.x) * x_fraction + x_offset,
                    min.y + (max.y - min.y) * y_fraction,
                    min.z + (max.z - min.z) * z_fraction + z_offset,
                );
                if explosion_ray_is_clear(from, center, collision_boxes) {
                    clear += 1;
                }
                total += 1;
                z_fraction += z_step;
            }
            y_fraction += y_step;
        }
        x_fraction += x_step;
    }

    clear as f32 / total as f32
}

fn explosion_ray_is_clear(
    from: Vec3,
    to: Vec3,
    collision_boxes: &mut impl FnMut(BlockPos) -> Option<Vec<[f64; 6]>>,
) -> bool {
    let min_x = from.x.min(to.x).floor() as i32;
    let max_x = from.x.max(to.x).floor() as i32;
    let min_y = (from.y.min(to.y).floor() as i32).saturating_sub(1);
    let max_y = from.y.max(to.y).floor() as i32;
    let min_z = from.z.min(to.z).floor() as i32;
    let max_z = from.z.max(to.z).floor() as i32;

    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                let Some(boxes) = collision_boxes(BlockPos { x, y, z }) else {
                    return false;
                };
                for collision_box in boxes {
                    let world_box = [
                        f64::from(x) + collision_box[0],
                        f64::from(y) + collision_box[1],
                        f64::from(z) + collision_box[2],
                        f64::from(x) + collision_box[3],
                        f64::from(y) + collision_box[4],
                        f64::from(z) + collision_box[5],
                    ];
                    if segment_intersects_box(from, to, world_box) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn segment_intersects_box(from: Vec3, to: Vec3, bounds: [f64; 6]) -> bool {
    let direction = Vec3::new(to.x - from.x, to.y - from.y, to.z - from.z);
    let mut enter = 0.0_f64;
    let mut exit = 1.0_f64;
    for (origin, delta, min, max) in [
        (from.x, direction.x, bounds[0], bounds[3]),
        (from.y, direction.y, bounds[1], bounds[4]),
        (from.z, direction.z, bounds[2], bounds[5]),
    ] {
        if delta.abs() < f64::EPSILON {
            if origin < min || origin > max {
                return false;
            }
            continue;
        }
        let first = (min - origin) / delta;
        let second = (max - origin) / delta;
        enter = enter.max(first.min(second));
        exit = exit.min(first.max(second));
        if exit < enter {
            return false;
        }
    }
    exit > 1.0e-9 && enter <= 1.0
}

#[derive(Debug, Clone)]
pub(super) struct TntIgnitionPlan {
    pub(super) tnt: BlockEditPrecondition,
    pub(super) air: BlockStateId,
    pub(super) game_mode: GameMode,
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
    pub(super) flint_and_steel_max_damage: i32,
    pub(super) tnt_entity_type_id: i32,
}

#[derive(Debug)]
pub(super) struct CommittedTntIgnition {
    pub(super) block: BlockEditBatchOutcome,
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORACLE_SEED: i64 = 0x1234_ABCD;
    const CENTER: Vec3 = Vec3 {
        x: 0.5,
        y: 64.0,
        z: 0.5,
    };

    fn empty_sample(_: BlockPos) -> Option<ExplosionBlockSample> {
        Some(ExplosionBlockSample {
            resistance: None,
            explodable: true,
        })
    }

    #[test]
    fn java_legacy_random_matches_java_next_float_vector() {
        let mut random = JavaLegacyRandom::new(ORACLE_SEED);
        let actual = std::array::from_fn::<_, 8, _>(|_| random.next_float().to_bits());

        assert_eq!(
            actual,
            [
                0x3e1d_6b74,
                0x3f38_53a7,
                0x3f48_59ec,
                0x3f17_c20a,
                0x3f23_279d,
                0x3dcb_1268,
                0x3e9e_3030,
                0x3a36_7000,
            ]
        );
    }

    #[test]
    fn java_legacy_random_matches_java_chain_tnt_draws() {
        let mut random = JavaLegacyRandom::new(ORACLE_SEED);

        assert_eq!(random.next_double().to_bits(), 0x3fc3_ad6e_b70a_74fc);
        assert_eq!(random.next_int(20), 17);
    }

    #[test]
    fn shell_consumes_exactly_1352_random_draws() {
        let mut actual = JavaLegacyRandom::new(ORACLE_SEED);
        let mut expected = JavaLegacyRandom::new(ORACLE_SEED);
        for _ in 0..1_352 {
            expected.next_float();
        }

        let candidates = plan_explosion_candidates(CENTER, 4.0, &mut actual, |_| None).unwrap();

        assert!(candidates.is_empty());
        assert_eq!(actual.seed, expected.seed);
    }

    #[test]
    fn unavailable_center_stops_each_ray_immediately() {
        let mut random = JavaLegacyRandom::new(ORACLE_SEED);
        let mut samples = 0;

        let candidates = plan_explosion_candidates(CENTER, 4.0, &mut random, |_| {
            samples += 1;
            None
        })
        .unwrap();

        assert!(candidates.is_empty());
        assert_eq!(samples, 1_352);
    }

    #[test]
    fn high_resistance_excludes_center() {
        let mut random = JavaLegacyRandom::new(ORACLE_SEED);
        let center = BlockPos { x: 0, y: 64, z: 0 };

        let candidates = plan_explosion_candidates(CENTER, 4.0, &mut random, |_| {
            Some(ExplosionBlockSample {
                resistance: Some(100.0),
                explodable: true,
            })
        })
        .unwrap();

        assert!(!candidates.contains(&center));
    }

    #[test]
    fn empty_resistance_includes_center() {
        let mut random = JavaLegacyRandom::new(ORACLE_SEED);

        let candidates = plan_explosion_candidates(CENTER, 4.0, &mut random, empty_sample).unwrap();

        assert!(candidates.contains(&BlockPos { x: 0, y: 64, z: 0 }));
    }

    #[test]
    fn radius_four_candidates_match_complete_java_oracle_set() {
        let mut random = JavaLegacyRandom::new(ORACLE_SEED);

        let candidates = plan_explosion_candidates(CENTER, 4.0, &mut random, empty_sample).unwrap();
        let mut sorted = candidates.iter().copied().collect::<Vec<_>>();
        sorted.sort_unstable_by_key(|position| (position.x, position.y, position.z));
        let mut digest = 0xcbf2_9ce4_8422_2325_u64;
        for position in &sorted {
            for byte in format!("{},{},{}\n", position.x, position.y, position.z).bytes() {
                digest ^= u64::from(byte);
                digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }

        assert_eq!(candidates.len(), 1_152);
        assert_eq!(digest, 0x4db1_e009_cc12_cb48);
        assert_eq!(candidates.iter().map(|pos| pos.x).min(), Some(-7));
        assert_eq!(candidates.iter().map(|pos| pos.x).max(), Some(7));
        assert_eq!(candidates.iter().map(|pos| pos.y).min(), Some(57));
        assert_eq!(candidates.iter().map(|pos| pos.y).max(), Some(70));
        assert_eq!(candidates.iter().map(|pos| pos.z).min(), Some(-6));
        assert_eq!(candidates.iter().map(|pos| pos.z).max(), Some(7));
        for cardinal in [
            BlockPos { x: -7, y: 64, z: 0 },
            BlockPos { x: 0, y: 57, z: 0 },
            BlockPos { x: 0, y: 64, z: -6 },
        ] {
            assert!(candidates.contains(&cardinal), "missing {cardinal:?}");
        }
    }

    #[test]
    fn invalid_radius_and_center_are_rejected() {
        for radius in [0.0, -1.0, f32::NAN, f32::INFINITY, 64.000_01] {
            let mut random = JavaLegacyRandom::new(ORACLE_SEED);
            assert_eq!(
                plan_explosion_candidates(CENTER, radius, &mut random, empty_sample),
                Err(ExplosionPlannerError::InvalidRadius)
            );
        }

        let mut random = JavaLegacyRandom::new(ORACLE_SEED);
        assert_eq!(
            plan_explosion_candidates(
                Vec3 {
                    x: f64::NAN,
                    ..CENTER
                },
                4.0,
                &mut random,
                empty_sample,
            ),
            Err(ExplosionPlannerError::InvalidCenter)
        );
    }

    #[test]
    fn unobstructed_player_impact_matches_vanilla_formula() {
        let impact = plan_player_explosion_impact(
            Vec3::new(0.5, 64.06125, 0.5),
            4.0,
            Vec3::new(2.5, 64.0, 0.5),
            |_| Some(Vec::new()),
        )
        .expect("player is inside double-radius range");

        assert!((impact.exposure - 1.0).abs() < f32::EPSILON);
        assert!((impact.damage - 37.741_795).abs() < 0.000_1);
        assert!((impact.knockback.x - 0.591_463_5).abs() < 0.000_001);
        assert!((impact.knockback.y - 0.460_971_9).abs() < 0.000_001);
        assert!(impact.knockback.z.abs() < f64::EPSILON);
    }

    #[test]
    fn unobstructed_mob_impact_uses_entity_position_eye_and_aabb() {
        let impact = plan_entity_explosion_impact(
            Vec3::new(0.5, 64.06125, 0.5),
            4.0,
            Vec3::new(2.5, 64.0, 0.5),
            Vec3::new(2.5, 64.6, 0.5),
            Vec3::new(2.3, 64.0, 0.3),
            Vec3::new(2.7, 64.7, 0.7),
            |_| Some(Vec::new()),
        )
        .expect("mob is inside double-radius range");

        assert_eq!(impact.exposure, 1.0);
        assert!((impact.damage - 37.741_795).abs() < 0.000_1);
        assert!(impact.knockback.x > 0.0);
        assert!(impact.knockback.y > 0.0);
        assert!(impact.knockback.z.abs() < f64::EPSILON);
    }

    #[test]
    fn full_wall_reduces_mob_impact_to_base_damage() {
        let impact = plan_entity_explosion_impact(
            Vec3::new(0.5, 64.06125, 0.5),
            4.0,
            Vec3::new(2.5, 64.0, 0.5),
            Vec3::new(2.5, 64.6, 0.5),
            Vec3::new(2.3, 64.0, 0.3),
            Vec3::new(2.7, 64.7, 0.7),
            |position| {
                Some(
                    if position.x == 1 && position.z == 0 && (64..=65).contains(&position.y) {
                        vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
                    } else {
                        Vec::new()
                    },
                )
            },
        )
        .expect("mob is inside double-radius range");

        assert_eq!(impact.exposure, 0.0);
        assert_eq!(impact.damage, 1.0);
        assert_eq!(impact.knockback, Vec3::ZERO);
    }

    #[test]
    fn mob_outside_double_radius_is_not_affected() {
        assert_eq!(
            plan_entity_explosion_impact(
                Vec3::new(0.5, 64.06125, 0.5),
                4.0,
                Vec3::new(8.51, 64.06125, 0.5),
                Vec3::new(8.51, 64.66125, 0.5),
                Vec3::new(8.31, 64.06125, 0.3),
                Vec3::new(8.71, 64.76125, 0.7),
                |_| Some(Vec::new()),
            ),
            None
        );
    }

    #[test]
    fn full_wall_reduces_player_impact_to_base_damage() {
        let impact = plan_player_explosion_impact(
            Vec3::new(0.5, 64.06125, 0.5),
            4.0,
            Vec3::new(2.5, 64.0, 0.5),
            |position| {
                Some(
                    if position.x == 1 && position.z == 0 && (64..=66).contains(&position.y) {
                        vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
                    } else {
                        Vec::new()
                    },
                )
            },
        )
        .expect("player is inside double-radius range");

        assert_eq!(impact.exposure, 0.0);
        assert_eq!(impact.damage, 1.0);
        assert_eq!(impact.knockback, Vec3::ZERO);
    }

    #[test]
    fn player_outside_double_radius_is_not_affected() {
        assert_eq!(
            plan_player_explosion_impact(
                Vec3::new(0.5, 64.06125, 0.5),
                4.0,
                Vec3::new(8.51, 64.06125, 0.5),
                |_| Some(Vec::new()),
            ),
            None
        );
    }

    #[test]
    fn exposure_ray_sees_overheight_collision_rooted_below() {
        let clear = explosion_ray_is_clear(
            Vec3::new(0.5, 65.25, 0.5),
            Vec3::new(2.5, 65.25, 0.5),
            &mut |position| {
                Some(if position == (BlockPos { x: 1, y: 64, z: 0 }) {
                    vec![[0.0, 0.0, 0.0, 1.0, 1.5, 1.0]]
                } else {
                    Vec::new()
                })
            },
        );

        assert!(!clear);
    }

    #[test]
    fn invalid_resistance_is_rejected() {
        for resistance in [-0.001, f32::NAN, f32::INFINITY] {
            let mut random = JavaLegacyRandom::new(ORACLE_SEED);
            assert_eq!(
                plan_explosion_candidates(CENTER, 4.0, &mut random, |_| {
                    Some(ExplosionBlockSample {
                        resistance: Some(resistance),
                        explodable: true,
                    })
                }),
                Err(ExplosionPlannerError::InvalidResistance)
            );
        }
    }
}
