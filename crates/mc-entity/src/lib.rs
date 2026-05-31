//! # mc-entity
//!
//! Entity system, AI, pathfinding.
//!
//! Part of the Solaris engine.

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::TAU;
use std::ops::Range;

use uuid::Uuid;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stable runtime entity id used by the server and vanilla protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(pub i32);

/// 3D vector for entity positions and velocities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub fn horizontal_len(self) -> f64 {
        self.x.hypot(self.z)
    }

    #[must_use]
    pub fn horizontal_normalized(self) -> Self {
        let len = self.horizontal_len();
        if len <= f64::EPSILON {
            Self::ZERO
        } else {
            Self {
                x: self.x / len,
                y: 0.0,
                z: self.z / len,
            }
        }
    }
}

/// Entity yaw/pitch/head-yaw in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rotation {
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
}

impl Rotation {
    pub const ZERO: Self = Self {
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityLifecycle {
    Alive,
    Despawning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityItemStack {
    pub item_id: u32,
    pub count: i32,
    pub damage: Option<i32>,
}

impl EntityItemStack {
    #[must_use]
    pub const fn new(item_id: u32, count: i32) -> Self {
        Self {
            item_id,
            count,
            damage: None,
        }
    }

    #[must_use]
    pub const fn with_damage(mut self, damage: i32) -> Self {
        self.damage = Some(damage);
        self
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.count <= 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpawnEntity {
    pub uuid: Option<Uuid>,
    pub type_id: i32,
    pub type_name: String,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub item_stack: Option<EntityItemStack>,
    pub experience_value: Option<i32>,
    pub block_state: Option<u32>,
    pub attributes: AttributeSet,
    pub goal: GoalState,
}

impl SpawnEntity {
    #[must_use]
    pub fn new(type_id: i32, type_name: impl Into<String>, position: Vec3) -> Self {
        Self {
            uuid: None,
            type_id,
            type_name: type_name.into(),
            position,
            rotation: Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            attributes: AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntitySnapshot {
    pub id: EntityId,
    pub uuid: Uuid,
    pub type_id: i32,
    pub type_name: String,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub item_stack: Option<EntityItemStack>,
    pub experience_value: Option<i32>,
    pub block_state: Option<u32>,
    pub lifecycle: EntityLifecycle,
    pub health: f32,
    pub attributes: AttributeSet,
    pub goal: GoalState,
}

#[derive(Debug, Clone, Copy)]
pub struct EntityView<'a> {
    pub id: EntityId,
    pub uuid: Uuid,
    pub type_id: i32,
    pub type_name: &'a str,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub item_stack: Option<EntityItemStack>,
    pub experience_value: Option<i32>,
    pub block_state: Option<u32>,
    pub lifecycle: EntityLifecycle,
    pub health: f32,
    pub attributes: &'a AttributeSet,
    pub goal: &'a GoalState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityDamage {
    pub snapshot: EntitySnapshot,
    pub killed: bool,
}

/// Small vanilla attribute subset needed before real mob AI/combat.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttributeKind {
    MaxHealth,
    MovementSpeed,
    FollowRange,
    AttackDamage,
    Custom(String),
}

impl AttributeKind {
    #[must_use]
    pub fn vanilla_name(&self) -> &str {
        match self {
            Self::MaxHealth => "minecraft:max_health",
            Self::MovementSpeed => "minecraft:movement_speed",
            Self::FollowRange => "minecraft:follow_range",
            Self::AttackDamage => "minecraft:attack_damage",
            Self::Custom(name) => name.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttributeValue {
    pub base: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AttributeSet {
    values: BTreeMap<AttributeKind, AttributeValue>,
}

impl AttributeSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn vanilla_mob_defaults() -> Self {
        let mut attrs = Self::new();
        attrs.set_base(AttributeKind::MaxHealth, 20.0);
        attrs.set_base(AttributeKind::MovementSpeed, 0.25);
        attrs.set_base(AttributeKind::FollowRange, 16.0);
        attrs.set_base(AttributeKind::AttackDamage, 0.0);
        attrs
    }

    pub fn set_base(&mut self, kind: AttributeKind, base: f64) {
        self.values.insert(kind, AttributeValue { base });
    }

    #[must_use]
    pub fn base(&self, kind: &AttributeKind) -> Option<f64> {
        self.values.get(kind).map(|value| value.base)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&AttributeKind, &AttributeValue)> {
        self.values.iter()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GoalState {
    Idle,
    Wander {
        speed: f64,
        period_ticks: u32,
    },
    AquaticWander {
        speed: f64,
        vertical_speed: f64,
        period_ticks: u32,
    },
    FollowTarget {
        target: EntityId,
        speed: f64,
    },
    FollowPosition {
        target: Vec3,
        speed: f64,
    },
}

/// Observable counters from an AI goal tick.
///
/// These counters intentionally describe the read-only decision/application
/// boundary: every alive entity with a goal produces one applied decision, while
/// despawning entities are skipped and missing follow targets are reported
/// without mutating unrelated entities.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GoalTickStats {
    pub alive_entities: usize,
    pub decisions_applied: usize,
    pub skipped_non_alive: usize,
    pub missing_follow_targets: usize,
}

/// Dense entity storage. Hot state is kept in parallel vectors so later
/// milestones can split ticks into independent batches without redesigning
/// the runtime around per-entity locks.
#[derive(Debug, Default)]
pub struct EntityStore {
    next_id: i32,
    slots_by_id: HashMap<EntityId, usize>,
    ids: Vec<EntityId>,
    uuids: Vec<Uuid>,
    type_ids: Vec<i32>,
    type_names: Vec<String>,
    positions: Vec<Vec3>,
    rotations: Vec<Rotation>,
    velocities: Vec<Vec3>,
    on_ground: Vec<bool>,
    item_stacks: Vec<Option<EntityItemStack>>,
    experience_values: Vec<Option<i32>>,
    block_states: Vec<Option<u32>>,
    lifecycles: Vec<EntityLifecycle>,
    healths: Vec<f32>,
    attributes: Vec<AttributeSet>,
    goals: Vec<GoalState>,
}

impl EntityStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_next_id(next_id: i32) -> Self {
        Self {
            next_id,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    #[must_use]
    pub fn batch_ranges(&self, batch_size: usize) -> Vec<Range<usize>> {
        let batch_size = batch_size.max(1);
        (0..self.len())
            .step_by(batch_size)
            .map(|start| start..(start + batch_size).min(self.len()))
            .collect()
    }

    pub fn spawn(&mut self, entity: SpawnEntity) -> EntityId {
        let id = self.allocate_id();
        let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
        let slot = self.ids.len();
        self.slots_by_id.insert(id, slot);
        self.ids.push(id);
        self.uuids.push(uuid);
        self.type_ids.push(entity.type_id);
        self.type_names.push(entity.type_name);
        self.positions.push(entity.position);
        self.rotations.push(entity.rotation);
        self.velocities.push(entity.velocity);
        self.on_ground.push(entity.on_ground);
        self.item_stacks.push(entity.item_stack);
        self.experience_values.push(entity.experience_value);
        self.block_states.push(entity.block_state);
        self.lifecycles.push(EntityLifecycle::Alive);
        let health = entity
            .attributes
            .base(&AttributeKind::MaxHealth)
            .unwrap_or(20.0)
            .max(1.0) as f32;
        self.healths.push(health);
        self.attributes.push(entity.attributes);
        self.goals.push(entity.goal);
        id
    }

    #[must_use]
    pub fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.uuids.contains(&uuid)
    }

    pub fn insert_snapshot(&mut self, snapshot: EntitySnapshot) -> bool {
        if self.slots_by_id.contains_key(&snapshot.id) {
            return false;
        }
        let slot = self.ids.len();
        self.next_id = self.next_id.max(snapshot.id.0);
        self.slots_by_id.insert(snapshot.id, slot);
        self.ids.push(snapshot.id);
        self.uuids.push(snapshot.uuid);
        self.type_ids.push(snapshot.type_id);
        self.type_names.push(snapshot.type_name);
        self.positions.push(snapshot.position);
        self.rotations.push(snapshot.rotation);
        self.velocities.push(snapshot.velocity);
        self.on_ground.push(snapshot.on_ground);
        self.item_stacks.push(snapshot.item_stack);
        self.experience_values.push(snapshot.experience_value);
        self.block_states.push(snapshot.block_state);
        self.lifecycles.push(snapshot.lifecycle);
        self.healths.push(snapshot.health);
        self.attributes.push(snapshot.attributes);
        self.goals.push(snapshot.goal);
        true
    }

    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.slots_by_id.contains_key(&id)
    }

    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        self.slots_by_id
            .get(&id)
            .map(|&slot| self.snapshot_slot(slot))
    }

    #[must_use]
    pub fn view(&self, id: EntityId) -> Option<EntityView<'_>> {
        self.slots_by_id.get(&id).map(|&slot| self.view_slot(slot))
    }

    pub fn snapshots(&self) -> impl Iterator<Item = EntitySnapshot> + '_ {
        (0..self.len()).map(|slot| self.snapshot_slot(slot))
    }

    pub fn views(&self) -> impl Iterator<Item = EntityView<'_>> + '_ {
        (0..self.len()).map(|slot| self.view_slot(slot))
    }

    pub fn mark_despawning(&mut self, id: EntityId) -> bool {
        let Some(&slot) = self.slots_by_id.get(&id) else {
            return false;
        };
        self.lifecycles[slot] = EntityLifecycle::Despawning;
        true
    }

    pub fn remove(&mut self, id: EntityId) -> Option<EntitySnapshot> {
        let slot = self.slots_by_id.remove(&id)?;
        let removed = self.snapshot_slot(slot);
        self.swap_remove_slot(slot);
        Some(removed)
    }

    pub fn set_position(&mut self, id: EntityId, position: Vec3) -> bool {
        let Some(&slot) = self.slots_by_id.get(&id) else {
            return false;
        };
        self.positions[slot] = position;
        true
    }

    pub fn set_velocity(&mut self, id: EntityId, velocity: Vec3) -> bool {
        let Some(&slot) = self.slots_by_id.get(&id) else {
            return false;
        };
        self.velocities[slot] = velocity;
        true
    }

    pub fn set_on_ground(&mut self, id: EntityId, on_ground: bool) -> bool {
        let Some(&slot) = self.slots_by_id.get(&id) else {
            return false;
        };
        self.on_ground[slot] = on_ground;
        true
    }

    pub fn set_item_stack(&mut self, id: EntityId, item_stack: Option<EntityItemStack>) -> bool {
        let Some(&slot) = self.slots_by_id.get(&id) else {
            return false;
        };
        self.item_stacks[slot] = item_stack;
        true
    }

    pub fn set_goal(&mut self, id: EntityId, goal: GoalState) -> bool {
        let Some(&slot) = self.slots_by_id.get(&id) else {
            return false;
        };
        self.goals[slot] = goal;
        true
    }

    pub fn damage(&mut self, id: EntityId, amount: f32) -> Option<EntityDamage> {
        let slot = *self.slots_by_id.get(&id)?;
        if self.lifecycles[slot] != EntityLifecycle::Alive {
            return None;
        }
        self.healths[slot] = (self.healths[slot] - amount.max(0.0)).max(0.0);
        let killed = self.healths[slot] <= 0.0;
        if killed {
            self.lifecycles[slot] = EntityLifecycle::Despawning;
        }
        Some(EntityDamage {
            snapshot: self.snapshot_slot(slot),
            killed,
        })
    }

    pub fn attributes_mut(&mut self, id: EntityId) -> Option<&mut AttributeSet> {
        let slot = *self.slots_by_id.get(&id)?;
        self.attributes.get_mut(slot)
    }

    pub fn tick_goals(&mut self, tick: u64) {
        let _ = self.tick_goals_with_stats(tick);
    }

    pub fn tick_goals_with_stats(&mut self, tick: u64) -> GoalTickStats {
        let mut stats = GoalTickStats::default();
        for slot in 0..self.len() {
            if self.lifecycles[slot] != EntityLifecycle::Alive {
                stats.skipped_non_alive += 1;
                continue;
            }
            stats.alive_entities += 1;
            match self.goals[slot].clone() {
                GoalState::Idle => {
                    self.velocities[slot].x = 0.0;
                    self.velocities[slot].z = 0.0;
                }
                GoalState::Wander {
                    speed,
                    period_ticks,
                } => {
                    let period = u64::from(period_ticks.max(1));
                    let angle = deterministic_angle(self.ids[slot], tick / period);
                    self.velocities[slot].x = angle.cos() * speed;
                    self.velocities[slot].z = angle.sin() * speed;
                    self.rotations[slot].yaw = yaw_from_velocity(self.velocities[slot]);
                    self.rotations[slot].head_yaw = self.rotations[slot].yaw;
                }
                GoalState::AquaticWander {
                    speed,
                    vertical_speed,
                    period_ticks,
                } => {
                    let period = u64::from(period_ticks.max(1));
                    let phase = tick / period;
                    let angle = deterministic_angle(self.ids[slot], phase);
                    let vertical_wave = deterministic_wave(self.ids[slot], phase);
                    self.velocities[slot].x = angle.cos() * speed;
                    self.velocities[slot].z = angle.sin() * speed;
                    self.velocities[slot].y = vertical_wave * vertical_speed;
                    self.on_ground[slot] = false;
                    self.rotations[slot] = aquatic_rotation_from_velocity(self.velocities[slot]);
                }
                GoalState::FollowTarget { target, speed } => {
                    let velocity = if let Some(&target_slot) = self.slots_by_id.get(&target) {
                        Vec3 {
                            x: self.positions[target_slot].x - self.positions[slot].x,
                            y: 0.0,
                            z: self.positions[target_slot].z - self.positions[slot].z,
                        }
                        .horizontal_normalized()
                    } else {
                        stats.missing_follow_targets += 1;
                        Vec3::ZERO
                    };
                    self.velocities[slot].x = velocity.x * speed;
                    self.velocities[slot].z = velocity.z * speed;
                    self.rotations[slot].yaw = yaw_from_velocity(self.velocities[slot]);
                    self.rotations[slot].head_yaw = self.rotations[slot].yaw;
                }
                GoalState::FollowPosition { target, speed } => {
                    let velocity = Vec3 {
                        x: target.x - self.positions[slot].x,
                        y: 0.0,
                        z: target.z - self.positions[slot].z,
                    }
                    .horizontal_normalized();
                    self.velocities[slot].x = velocity.x * speed;
                    self.velocities[slot].z = velocity.z * speed;
                    self.rotations[slot].yaw = yaw_from_velocity(self.velocities[slot]);
                    self.rotations[slot].head_yaw = self.rotations[slot].yaw;
                }
            }
            stats.decisions_applied += 1;
        }
        stats
    }

    pub fn tick_positions(&mut self, delta_seconds: f64) {
        self.tick_positions_in_range(0..self.len(), delta_seconds);
    }

    pub fn tick_positions_in_range(&mut self, range: Range<usize>, delta_seconds: f64) {
        assert!(range.end <= self.len(), "entity tick range out of bounds");
        for slot in range {
            if self.lifecycles[slot] != EntityLifecycle::Alive {
                continue;
            }
            self.positions[slot].x += self.velocities[slot].x * delta_seconds;
            self.positions[slot].y += self.velocities[slot].y * delta_seconds;
            self.positions[slot].z += self.velocities[slot].z * delta_seconds;
        }
    }

    fn allocate_id(&mut self) -> EntityId {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        EntityId(self.next_id)
    }

    fn snapshot_slot(&self, slot: usize) -> EntitySnapshot {
        EntitySnapshot {
            id: self.ids[slot],
            uuid: self.uuids[slot],
            type_id: self.type_ids[slot],
            type_name: self.type_names[slot].clone(),
            position: self.positions[slot],
            rotation: self.rotations[slot],
            velocity: self.velocities[slot],
            on_ground: self.on_ground[slot],
            item_stack: self.item_stacks[slot],
            experience_value: self.experience_values[slot],
            block_state: self.block_states[slot],
            lifecycle: self.lifecycles[slot],
            health: self.healths[slot],
            attributes: self.attributes[slot].clone(),
            goal: self.goals[slot].clone(),
        }
    }

    fn view_slot(&self, slot: usize) -> EntityView<'_> {
        EntityView {
            id: self.ids[slot],
            uuid: self.uuids[slot],
            type_id: self.type_ids[slot],
            type_name: &self.type_names[slot],
            position: self.positions[slot],
            rotation: self.rotations[slot],
            velocity: self.velocities[slot],
            on_ground: self.on_ground[slot],
            item_stack: self.item_stacks[slot],
            experience_value: self.experience_values[slot],
            block_state: self.block_states[slot],
            lifecycle: self.lifecycles[slot],
            health: self.healths[slot],
            attributes: &self.attributes[slot],
            goal: &self.goals[slot],
        }
    }

    fn swap_remove_slot(&mut self, slot: usize) {
        self.ids.swap_remove(slot);
        self.uuids.swap_remove(slot);
        self.type_ids.swap_remove(slot);
        self.type_names.swap_remove(slot);
        self.positions.swap_remove(slot);
        self.rotations.swap_remove(slot);
        self.velocities.swap_remove(slot);
        self.on_ground.swap_remove(slot);
        self.item_stacks.swap_remove(slot);
        self.experience_values.swap_remove(slot);
        self.block_states.swap_remove(slot);
        self.lifecycles.swap_remove(slot);
        self.healths.swap_remove(slot);
        self.attributes.swap_remove(slot);
        self.goals.swap_remove(slot);

        if slot < self.ids.len() {
            self.slots_by_id.insert(self.ids[slot], slot);
        }
    }
}

fn deterministic_uuid(id: EntityId) -> Uuid {
    let low = id.0 as u32 as u128;
    Uuid::from_u128(0x5f1a_0000_0000_0000_0000_0000_0000_0000 | low)
}

fn deterministic_angle(id: EntityId, phase: u64) -> f64 {
    let mixed = splitmix64((id.0 as u32 as u64) ^ phase.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    (mixed as f64 / u64::MAX as f64) * TAU
}

fn deterministic_wave(id: EntityId, phase: u64) -> f64 {
    deterministic_angle(id, phase.wrapping_add(0x41)).sin()
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn yaw_from_velocity(velocity: Vec3) -> f32 {
    if velocity.x == 0.0 && velocity.z == 0.0 {
        0.0
    } else {
        velocity.z.atan2(velocity.x).to_degrees() as f32 - 90.0
    }
}

fn aquatic_rotation_from_velocity(velocity: Vec3) -> Rotation {
    let yaw = yaw_from_velocity(velocity) + 90.0;
    let horizontal = velocity.horizontal_len();
    let pitch = if horizontal <= f64::EPSILON && velocity.y == 0.0 {
        0.0
    } else {
        (-velocity.y).atan2(horizontal).to_degrees() as f32
    };
    Rotation {
        yaw,
        pitch: pitch.clamp(-35.0, 35.0),
        head_yaw: yaw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cow(position: Vec3) -> SpawnEntity {
        SpawnEntity::new(144, "minecraft:cow", position)
    }

    #[test]
    fn spawn_assigns_stable_dense_ids() {
        let mut store = EntityStore::new();
        let a = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let b = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));

        assert_eq!(a, EntityId(1));
        assert_eq!(b, EntityId(2));
        assert_eq!(store.len(), 2);
        assert!(store.contains(a));
        assert_eq!(
            store.snapshot(b).unwrap().position,
            Vec3::new(2.0, 64.0, 2.0)
        );
    }

    #[test]
    fn item_stack_payload_round_trips_through_snapshots() {
        let mut store = EntityStore::new();
        let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.5, 0.5));
        item.item_stack = Some(EntityItemStack::new(42, 3));

        let id = store.spawn(item);

        assert_eq!(
            store.snapshot(id).and_then(|snapshot| snapshot.item_stack),
            Some(EntityItemStack::new(42, 3))
        );
        assert!(store.set_item_stack(id, Some(EntityItemStack::new(42, 1))));
        assert_eq!(
            store.snapshot(id).and_then(|snapshot| snapshot.item_stack),
            Some(EntityItemStack::new(42, 1))
        );
    }

    #[test]
    fn damage_reduces_health_and_marks_killed_entities() {
        let mut store = EntityStore::new();
        let id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));

        let hit = store.damage(id, 5.0).unwrap();
        assert!(!hit.killed);
        assert_eq!(hit.snapshot.health, 15.0);
        assert_eq!(hit.snapshot.lifecycle, EntityLifecycle::Alive);

        let lethal = store.damage(id, 20.0).unwrap();
        assert!(lethal.killed);
        assert_eq!(lethal.snapshot.health, 0.0);
        assert_eq!(lethal.snapshot.lifecycle, EntityLifecycle::Despawning);
    }

    #[test]
    fn remove_keeps_moved_slot_addressable() {
        let mut store = EntityStore::new();
        let a = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let b = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));
        let c = store.spawn(cow(Vec3::new(3.0, 64.0, 3.0)));

        let removed = store.remove(b).unwrap();

        assert_eq!(removed.id, b);
        assert!(!store.contains(b));
        assert!(store.contains(a));
        assert!(store.contains(c));
        assert_eq!(store.len(), 2);
        assert_eq!(
            store.snapshot(c).unwrap().position,
            Vec3::new(3.0, 64.0, 3.0)
        );
    }

    #[test]
    fn attributes_expose_vanilla_names_and_base_values() {
        let mut attrs = AttributeSet::vanilla_mob_defaults();
        attrs.set_base(AttributeKind::AttackDamage, 3.0);

        assert_eq!(
            AttributeKind::MaxHealth.vanilla_name(),
            "minecraft:max_health"
        );
        assert_eq!(attrs.base(&AttributeKind::MaxHealth), Some(20.0));
        assert_eq!(attrs.base(&AttributeKind::AttackDamage), Some(3.0));
        assert_eq!(attrs.iter().count(), 4);
    }

    #[test]
    fn position_ticks_can_be_split_into_batches() {
        let mut store = EntityStore::new();
        let a = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let b = store.spawn(cow(Vec3::new(10.0, 64.0, 10.0)));
        store.set_velocity(a, Vec3::new(1.0, 0.0, 0.0));
        store.set_velocity(b, Vec3::new(0.0, 0.0, 1.0));

        assert_eq!(store.batch_ranges(1), vec![0..1, 1..2]);
        store.tick_positions_in_range(0..1, 0.5);

        assert_eq!(
            store.snapshot(a).unwrap().position,
            Vec3::new(0.5, 64.0, 0.0)
        );
        assert_eq!(
            store.snapshot(b).unwrap().position,
            Vec3::new(10.0, 64.0, 10.0)
        );
    }

    #[test]
    fn wander_goal_is_deterministic() {
        let mut a = EntityStore::new();
        let mut b = EntityStore::new();
        let entity_a = a.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let entity_b = b.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        a.set_goal(
            entity_a,
            GoalState::Wander {
                speed: 0.2,
                period_ticks: 20,
            },
        );
        b.set_goal(
            entity_b,
            GoalState::Wander {
                speed: 0.2,
                period_ticks: 20,
            },
        );

        a.tick_goals(40);
        b.tick_goals(40);

        assert_eq!(
            a.snapshot(entity_a).unwrap().velocity,
            b.snapshot(entity_b).unwrap().velocity
        );
    }

    #[test]
    fn aquatic_wander_sets_3d_motion_and_pitch() {
        let mut store = EntityStore::new();
        let mut fish = SpawnEntity::new(2, "minecraft:cod", Vec3::new(0.0, 50.0, 0.0));
        fish.goal = GoalState::AquaticWander {
            speed: 0.2,
            vertical_speed: 0.1,
            period_ticks: 20,
        };
        let id = store.spawn(fish);

        store.tick_goals(40);
        let snapshot = store.snapshot(id).unwrap();

        assert!(snapshot.velocity.horizontal_len() > 0.0);
        assert!(snapshot.velocity.y.abs() > 0.0);
        assert!(!snapshot.on_ground);
        assert_ne!(snapshot.rotation.pitch, 0.0);
        assert_eq!(snapshot.rotation.yaw, snapshot.rotation.head_yaw);
    }

    #[test]
    fn follow_target_sets_horizontal_velocity() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let target = store.spawn(cow(Vec3::new(3.0, 64.0, 4.0)));
        store.set_goal(follower, GoalState::FollowTarget { target, speed: 0.5 });

        store.tick_goals(1);

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert!((velocity.x - 0.3).abs() < 0.000_001);
        assert!((velocity.z - 0.4).abs() < 0.000_001);
    }

    #[test]
    fn goal_tick_stats_report_applied_and_skipped_ai_decisions() {
        let mut store = EntityStore::new();
        let idle = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let follower = store.spawn(cow(Vec3::new(2.0, 64.0, 0.0)));
        let despawning = store.spawn(cow(Vec3::new(4.0, 64.0, 0.0)));
        store.set_velocity(idle, Vec3::new(1.0, 2.0, 3.0));
        store.set_goal(
            follower,
            GoalState::FollowTarget {
                target: EntityId(99_999),
                speed: 0.5,
            },
        );
        store.mark_despawning(despawning);

        let stats = store.tick_goals_with_stats(1);

        assert_eq!(
            stats,
            GoalTickStats {
                alive_entities: 2,
                decisions_applied: 2,
                skipped_non_alive: 1,
                missing_follow_targets: 1,
            }
        );
        assert_eq!(
            store.snapshot(idle).unwrap().velocity,
            Vec3::new(0.0, 2.0, 0.0)
        );
        assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
        assert_eq!(
            store.snapshot(despawning).unwrap().lifecycle,
            EntityLifecycle::Despawning
        );
    }

    #[test]
    fn follow_position_sets_horizontal_velocity() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(0.0, 65.0, 4.0),
                speed: 0.5,
            },
        );

        store.tick_goals(1);

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert_eq!(velocity.x, 0.0);
        assert!((velocity.z - 0.5).abs() < 0.000_001);
    }
}
