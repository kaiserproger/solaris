//! Bounded Java Edition 26.1.2 Ender Dragon air-combat kernel.
//!
//! This module intentionally owns only the Phase-4 D1 flying loop. Landing,
//! sitting/perch phases, crystals, the End fight, terrain griefing and boss-bar
//! presentation remain outside this kernel and must not be inferred from it.

use std::f64::consts::PI;

use crate::Vec3;

pub const STRAFE_FIREBALL_RANGE_SQ: f64 = 64.0 * 64.0;
pub const STRAFE_FIREBALL_CHARGE_TICKS: u8 = 5;
pub const STRAFE_FIREBALL_MAX_ANGLE_DEGREES: f64 = 10.0;
pub const CHARGE_RECOVERY_TICKS: u8 = 10;
pub const WING_CONTACT_DAMAGE: f32 = 5.0;
pub const HEAD_NECK_CONTACT_DAMAGE: f32 = 10.0;
pub const FIGHTLESS_DRAGON_XP: u32 = 500;
pub const D1_ORBIT_RADIUS: f64 = 20.0;
pub const D1_ORBIT_HEIGHT: f64 = 5.0;
pub const D1_ORBIT_POINTS: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragonAirPhase {
    HoldingPattern,
    StrafePlayer,
    ChargingPlayer,
    Dying,
}

impl DragonAirPhase {
    #[must_use]
    pub const fn fly_speed(self) -> f64 {
        match self {
            Self::HoldingPattern | Self::StrafePlayer => 0.6,
            Self::ChargingPlayer | Self::Dying => 3.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragonPart {
    Head,
    Neck,
    Body,
    Tail1,
    Tail2,
    Tail3,
    Wing1,
    Wing2,
}

impl DragonPart {
    #[must_use]
    pub const fn from_protocol_offset(offset: i32) -> Option<Self> {
        Some(match offset {
            1 => Self::Head,
            2 => Self::Neck,
            3 => Self::Body,
            4 => Self::Tail1,
            5 => Self::Tail2,
            6 => Self::Tail3,
            7 => Self::Wing1,
            8 => Self::Wing2,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn dimensions(self) -> DragonPartDimensions {
        match self {
            Self::Head => DragonPartDimensions::new(1.0, 1.0),
            Self::Neck => DragonPartDimensions::new(3.0, 3.0),
            Self::Body => DragonPartDimensions::new(5.0, 3.0),
            Self::Tail1 | Self::Tail2 | Self::Tail3 => DragonPartDimensions::new(2.0, 2.0),
            Self::Wing1 | Self::Wing2 => DragonPartDimensions::new(4.0, 2.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragonPartDimensions {
    pub width: f64,
    pub height: f64,
}

impl DragonPartDimensions {
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragonFlightSample {
    pub y: f64,
    pub yaw: f32,
}

impl DragonFlightSample {
    #[must_use]
    pub const fn new(y: f64, yaw: f32) -> Self {
        Self { y, yaw }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragonAirState {
    pub phase: DragonAirPhase,
    pub anchor: Vec3,
    pub orbit_index: u8,
    pub clockwise: bool,
    pub attack_cycle: u8,
    pub target_session: Option<u64>,
    pub target_entity_id: Option<i32>,
    pub fireball_charge: u8,
    pub charge_recovery_ticks: u8,
    pub fly_target: Option<Vec3>,
    pub yaw_accel: f32,
    pub death_time: u16,
    pub history: [DragonFlightSample; 17],
}

impl DragonAirState {
    #[must_use]
    pub fn new(anchor: Vec3, yaw: f32) -> Self {
        Self {
            phase: DragonAirPhase::HoldingPattern,
            anchor,
            orbit_index: 0,
            clockwise: false,
            attack_cycle: 0,
            target_session: None,
            target_entity_id: None,
            fireball_charge: 0,
            charge_recovery_ticks: 0,
            fly_target: Some(d1_orbit_waypoint(anchor, 0)),
            yaw_accel: 0.0,
            death_time: 0,
            history: [DragonFlightSample::new(anchor.y, yaw); 17],
        }
    }

    pub fn record_flight_sample(&mut self, y: f64, yaw: f32) {
        self.history.copy_within(0..16, 1);
        self.history[0] = DragonFlightSample::new(y, yaw);
    }

    #[must_use]
    pub fn sample(&self, delay: usize) -> DragonFlightSample {
        self.history[delay.min(16)]
    }

    pub fn clear_target(&mut self) {
        self.target_session = None;
        self.target_entity_id = None;
        self.fireball_charge = 0;
        self.charge_recovery_ticks = 0;
    }

    pub fn begin_strafe(&mut self, session: u64, entity_id: i32, target: Vec3) {
        self.phase = DragonAirPhase::StrafePlayer;
        self.target_session = Some(session);
        self.target_entity_id = Some(entity_id);
        self.fireball_charge = 0;
        self.fly_target = Some(target);
    }

    pub fn begin_charge(&mut self, session: u64, entity_id: i32, target: Vec3) {
        self.phase = DragonAirPhase::ChargingPlayer;
        self.target_session = Some(session);
        self.target_entity_id = Some(entity_id);
        self.charge_recovery_ticks = 0;
        self.fly_target = Some(target);
    }

    pub fn return_to_holding(&mut self) {
        self.phase = DragonAirPhase::HoldingPattern;
        self.clear_target();
        self.attack_cycle = self.attack_cycle.wrapping_add(1);
        self.orbit_index = next_orbit_index(self.orbit_index, self.clockwise);
        self.fly_target = Some(d1_orbit_waypoint(self.anchor, self.orbit_index));
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragonFlightMotion {
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub yaw_accel: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragonWingContact {
    pub push: Vec3,
    pub damage: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragonDeathStep {
    pub next_death_time: u16,
    pub xp_award: u32,
    pub remove: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragonStrafeStep {
    pub next_charge: u8,
    pub fire: bool,
}

#[must_use]
pub fn dragon_part_damage(
    phase: DragonAirPhase,
    part: DragonPart,
    incoming: f32,
    source_can_hurt_dragon: bool,
) -> Option<f32> {
    if phase == DragonAirPhase::Dying
        || !source_can_hurt_dragon
        || !incoming.is_finite()
        || incoming < 0.0
    {
        return None;
    }
    let damage = if part == DragonPart::Head {
        incoming
    } else {
        incoming / 4.0 + incoming.min(1.0)
    };
    (damage.is_finite() && damage >= 0.01).then_some(damage)
}

#[must_use]
pub fn wing_contact(dx: f64, dz: f64, can_damage: bool) -> Option<DragonWingContact> {
    if !dx.is_finite() || !dz.is_finite() {
        return None;
    }
    let distance_sq = (dx * dx + dz * dz).max(0.1);
    Some(DragonWingContact {
        push: Vec3::new(dx / distance_sq * 4.0, 0.2, dz / distance_sq * 4.0),
        damage: if can_damage { WING_CONTACT_DAMAGE } else { 0.0 },
    })
}

#[must_use]
pub fn strafe_step(
    current_charge: u8,
    target_distance_sq: f64,
    has_line_of_sight: bool,
    facing_angle_degrees: f64,
) -> DragonStrafeStep {
    if !target_distance_sq.is_finite()
        || !facing_angle_degrees.is_finite()
        || target_distance_sq >= STRAFE_FIREBALL_RANGE_SQ
        || !has_line_of_sight
    {
        return DragonStrafeStep {
            next_charge: current_charge.saturating_sub(1),
            fire: false,
        };
    }
    let next_charge = current_charge.saturating_add(1);
    let fire = next_charge >= STRAFE_FIREBALL_CHARGE_TICKS
        && (0.0..STRAFE_FIREBALL_MAX_ANGLE_DEGREES).contains(&facing_angle_degrees);
    DragonStrafeStep {
        next_charge: if fire { 0 } else { next_charge },
        fire,
    }
}

#[must_use]
pub fn charge_recovery_step(current: u8, arrived_or_collided: bool) -> (u8, bool) {
    let next = if arrived_or_collided {
        current.saturating_add(1)
    } else {
        0
    };
    (next, next >= CHARGE_RECOVERY_TICKS)
}

#[must_use]
pub fn dragon_facing_angle_degrees(yaw: f32, from: Vec3, target: Vec3) -> Option<f64> {
    if !yaw.is_finite() || !from.is_finite() || !target.is_finite() {
        return None;
    }
    let dx = target.x - from.x;
    let dz = target.z - from.z;
    let horizontal = dx.hypot(dz);
    if horizontal <= f64::EPSILON {
        return Some(0.0);
    }
    let aim_x = dx / horizontal;
    let aim_z = dz / horizontal;
    let yaw_rad = f64::from(yaw) * PI / 180.0;
    let forward_x = yaw_rad.sin();
    let forward_z = -yaw_rad.cos();
    let dot = (forward_x * aim_x + forward_z * aim_z).clamp(-1.0, 1.0);
    Some(dot.acos() * 180.0 / PI + 0.5)
}

#[must_use]
pub fn steer_flight(
    phase: DragonAirPhase,
    position: Vec3,
    velocity: Vec3,
    yaw: f32,
    yaw_accel: f32,
    target: Vec3,
) -> Option<DragonFlightMotion> {
    if !position.is_finite()
        || !velocity.is_finite()
        || !target.is_finite()
        || !yaw.is_finite()
        || !yaw_accel.is_finite()
    {
        return None;
    }
    let xdd = target.x - position.x;
    let mut ydd = target.y - position.y;
    let zdd = target.z - position.z;
    let distance_sq = xdd * xdd + ydd * ydd + zdd * zdd;
    if !distance_sq.is_finite() {
        return None;
    }
    let horizontal = xdd.hypot(zdd);
    if horizontal > 0.0 {
        ydd = (ydd / horizontal).clamp(-phase.fly_speed(), phase.fly_speed());
    }

    let mut next_velocity = velocity;
    next_velocity.y += ydd * 0.01;
    let yaw_rad = f64::from(wrap_degrees(yaw)) * PI / 180.0;
    let aim = normalized(Vec3::new(xdd, target.y - position.y, zdd))?;
    let dir = normalized(Vec3::new(yaw_rad.sin(), next_velocity.y, -yaw_rad.cos()))?;
    let alignment = ((dot(dir, aim) + 0.5) / 1.5).max(0.0);

    let mut next_yaw_accel = yaw_accel;
    let mut next_yaw = wrap_degrees(yaw);
    if xdd.abs() > 1.0e-5 || zdd.abs() > 1.0e-5 {
        let yaw_delta = wrap_degrees(180.0 - (xdd.atan2(zdd) * 180.0 / PI) as f32 - next_yaw)
            .clamp(-50.0, 50.0);
        next_yaw_accel *= 0.8;
        next_yaw_accel += yaw_delta * turn_speed(velocity.horizontal_len()) as f32;
        next_yaw = wrap_degrees(next_yaw + next_yaw_accel * 0.1);
    }

    let span = 2.0 / (distance_sq + 1.0);
    let thrust = 0.06 * (alignment * span + (1.0 - span));
    let next_yaw_rad = f64::from(next_yaw) * PI / 180.0;
    next_velocity.x += next_yaw_rad.sin() * thrust;
    next_velocity.z += -next_yaw_rad.cos() * thrust;

    let actual = normalized(next_velocity).unwrap_or(Vec3::ZERO);
    let slide = 0.8 + 0.15 * (dot(actual, dir) + 1.0) / 2.0;
    next_velocity.x *= slide;
    next_velocity.y *= 0.91;
    next_velocity.z *= slide;
    if !next_velocity.is_finite() {
        return None;
    }
    let next_position = Vec3::new(
        position.x + next_velocity.x,
        position.y + next_velocity.y,
        position.z + next_velocity.z,
    );
    next_position.is_finite().then_some(DragonFlightMotion {
        position: next_position,
        velocity: next_velocity,
        yaw: next_yaw,
        yaw_accel: next_yaw_accel,
    })
}

#[must_use]
pub fn part_center(
    state: &DragonAirState,
    dragon_position: Vec3,
    yaw: f32,
    part: DragonPart,
) -> Option<Vec3> {
    if !dragon_position.is_finite() || !yaw.is_finite() {
        return None;
    }
    let yaw_rad = f64::from(yaw) * PI / 180.0;
    let sin_yaw = yaw_rad.sin();
    let cos_yaw = yaw_rad.cos();
    let tilt = (state.sample(5).y - state.sample(10).y) * 10.0 * PI / 180.0;
    let cos_tilt = tilt.cos();
    let sin_tilt = tilt.sin();
    let offset = match part {
        DragonPart::Body => Vec3::new(sin_yaw * 0.5, 0.0, -cos_yaw * 0.5),
        DragonPart::Wing1 => Vec3::new(cos_yaw * 4.5, 2.0, sin_yaw * 4.5),
        DragonPart::Wing2 => Vec3::new(-cos_yaw * 4.5, 2.0, -sin_yaw * 4.5),
        DragonPart::Head | DragonPart::Neck => {
            let distance = if part == DragonPart::Head { 6.5 } else { 5.5 };
            let adjusted_yaw = yaw_rad - f64::from(state.yaw_accel) * 0.01;
            let head_y_offset = state.sample(5).y - state.sample(0).y;
            Vec3::new(
                adjusted_yaw.sin() * distance * cos_tilt,
                head_y_offset + sin_tilt * distance,
                -adjusted_yaw.cos() * distance * cos_tilt,
            )
        }
        DragonPart::Tail1 | DragonPart::Tail2 | DragonPart::Tail3 => {
            let index = match part {
                DragonPart::Tail1 => 0,
                DragonPart::Tail2 => 1,
                DragonPart::Tail3 => 2,
                _ => unreachable!(),
            };
            let p1 = state.sample(5);
            let p0 = state.sample(12 + index * 2);
            let wrapped = f64::from(wrap_degrees(p0.yaw - p1.yaw)) * PI / 180.0;
            let rot = yaw_rad + wrapped;
            let distance = f64::from((index + 1) as u32) * 2.0;
            Vec3::new(
                -(sin_yaw * 1.5 + rot.sin() * distance) * cos_tilt,
                p0.y - p1.y - (distance + 1.5) * sin_tilt + 1.5,
                (cos_yaw * 1.5 + rot.cos() * distance) * cos_tilt,
            )
        }
    };
    let center = Vec3::new(
        dragon_position.x + offset.x,
        dragon_position.y + offset.y,
        dragon_position.z + offset.z,
    );
    center.is_finite().then_some(center)
}

#[must_use]
pub fn d1_orbit_waypoint(anchor: Vec3, index: u8) -> Vec3 {
    let angle = f64::from(index % D1_ORBIT_POINTS) * 2.0 * PI / f64::from(D1_ORBIT_POINTS);
    Vec3::new(
        anchor.x + angle.cos() * D1_ORBIT_RADIUS,
        anchor.y + D1_ORBIT_HEIGHT,
        anchor.z + angle.sin() * D1_ORBIT_RADIUS,
    )
}

#[must_use]
pub const fn next_orbit_index(current: u8, clockwise: bool) -> u8 {
    if clockwise {
        (current + 1) % D1_ORBIT_POINTS
    } else if current == 0 {
        D1_ORBIT_POINTS - 1
    } else {
        current - 1
    }
}

#[must_use]
pub fn death_step(current_death_time: u16) -> DragonDeathStep {
    let next = current_death_time.saturating_add(1);
    let mut xp_award = 0;
    if next > 150 && next.is_multiple_of(5) {
        xp_award += (FIGHTLESS_DRAGON_XP as f64 * 0.08).floor() as u32;
    }
    let remove = next >= 200;
    if remove {
        xp_award += (FIGHTLESS_DRAGON_XP as f64 * 0.2).floor() as u32;
    }
    DragonDeathStep {
        next_death_time: next,
        xp_award,
        remove,
    }
}

#[must_use]
pub fn choose_d1_attack(attack_cycle: u8, target_distance_sq: f64) -> DragonAirPhase {
    if attack_cycle % 3 == 2 && target_distance_sq.is_finite() && target_distance_sq < 150.0 * 150.0
    {
        DragonAirPhase::ChargingPlayer
    } else {
        DragonAirPhase::StrafePlayer
    }
}

fn turn_speed(horizontal_speed: f64) -> f64 {
    let adjusted = horizontal_speed + 1.0;
    0.7 / adjusted.min(40.0) / adjusted
}

fn dot(left: Vec3, right: Vec3) -> f64 {
    left.x * right.x + left.y * right.y + left.z * right.z
}

fn normalized(value: Vec3) -> Option<Vec3> {
    let length_sq = dot(value, value);
    if !length_sq.is_finite() || length_sq <= 1.0e-14 {
        return None;
    }
    let inverse = length_sq.sqrt().recip();
    let value = Vec3::new(value.x * inverse, value.y * inverse, value.z * inverse);
    value.is_finite().then_some(value)
}

fn wrap_degrees(mut degrees: f32) -> f32 {
    degrees %= 360.0;
    if degrees >= 180.0 {
        degrees -= 360.0;
    }
    if degrees < -180.0 {
        degrees += 360.0;
    }
    degrees
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_non_head_damage_match_oracle_and_dying_is_immune() {
        assert_eq!(
            dragon_part_damage(DragonAirPhase::HoldingPattern, DragonPart::Head, 8.0, true),
            Some(8.0)
        );
        assert_eq!(
            dragon_part_damage(DragonAirPhase::HoldingPattern, DragonPart::Body, 8.0, true),
            Some(3.0)
        );
        assert_eq!(
            dragon_part_damage(DragonAirPhase::HoldingPattern, DragonPart::Head, 8.0, false),
            None
        );
        assert_eq!(
            dragon_part_damage(DragonAirPhase::Dying, DragonPart::Head, 8.0, true),
            None
        );
    }

    #[test]
    fn strafe_requires_five_visible_aligned_ticks_and_decays_without_los() {
        let mut charge = 0;
        for _ in 0..4 {
            let step = strafe_step(charge, 100.0, true, 5.0);
            assert!(!step.fire);
            charge = step.next_charge;
        }
        let step = strafe_step(charge, 100.0, true, 5.0);
        assert!(step.fire);
        assert_eq!(step.next_charge, 0);
        assert_eq!(strafe_step(4, 100.0, false, 5.0).next_charge, 3);
        assert_eq!(
            strafe_step(4, STRAFE_FIREBALL_RANGE_SQ, true, 5.0).next_charge,
            3
        );
        assert!(!strafe_step(4, 100.0, true, 10.0).fire);
    }

    #[test]
    fn charging_requires_ten_recovery_ticks_after_arrival() {
        let mut recovery = 0;
        for _ in 0..9 {
            let (next, done) = charge_recovery_step(recovery, true);
            recovery = next;
            assert!(!done);
        }
        assert!(charge_recovery_step(recovery, true).1);
        assert_eq!(charge_recovery_step(7, false), (0, false));
    }

    #[test]
    fn wing_push_matches_oracle() {
        let contact = wing_contact(2.0, 0.0, true).unwrap();
        assert_eq!(contact.push, Vec3::new(2.0, 0.2, 0.0));
        assert_eq!(contact.damage, 5.0);
    }

    #[test]
    fn steering_is_finite_and_yaw_turn_is_bounded() {
        let motion = steer_flight(
            DragonAirPhase::HoldingPattern,
            Vec3::ZERO,
            Vec3::ZERO,
            0.0,
            0.0,
            Vec3::new(20.0, 5.0, 0.0),
        )
        .unwrap();
        assert!(motion.position.is_finite());
        assert!(motion.velocity.is_finite());
        assert!(motion.yaw.is_finite());
        assert!(motion.yaw.abs() <= 50.0);
    }

    #[test]
    fn critical_part_offsets_and_dimensions_are_finite() {
        let state = DragonAirState::new(Vec3::new(0.0, 64.0, 0.0), 0.0);
        assert_eq!(
            DragonPart::Head.dimensions(),
            DragonPartDimensions::new(1.0, 1.0)
        );
        assert_eq!(
            DragonPart::Body.dimensions(),
            DragonPartDimensions::new(5.0, 3.0)
        );
        assert_eq!(
            DragonPart::Wing1.dimensions(),
            DragonPartDimensions::new(4.0, 2.0)
        );
        for part in [
            DragonPart::Head,
            DragonPart::Neck,
            DragonPart::Body,
            DragonPart::Tail1,
            DragonPart::Tail2,
            DragonPart::Tail3,
            DragonPart::Wing1,
            DragonPart::Wing2,
        ] {
            assert!(
                part_center(&state, Vec3::new(0.0, 64.0, 0.0), 0.0, part)
                    .unwrap()
                    .is_finite()
            );
        }
    }

    #[test]
    fn fightless_death_schedule_totals_five_hundred_xp_and_removes_at_two_hundred() {
        let mut time = 0;
        let mut xp = 0;
        let mut removed_at = None;
        while time < 200 {
            let step = death_step(time);
            time = step.next_death_time;
            xp += step.xp_award;
            if step.remove {
                removed_at = Some(time);
                break;
            }
        }
        assert_eq!(xp, 500);
        assert_eq!(removed_at, Some(200));
    }

    #[test]
    fn d1_phase_choices_never_enter_unimplemented_sitting_phases() {
        for cycle in 0..32 {
            assert!(matches!(
                choose_d1_attack(cycle, 100.0),
                DragonAirPhase::StrafePlayer | DragonAirPhase::ChargingPlayer
            ));
        }
    }
}
