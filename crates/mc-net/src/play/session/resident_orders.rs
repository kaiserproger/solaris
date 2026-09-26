//! Session/entity-owner paths for resident order execution (C4).
//!
//! The durable ledger is the order authority; this module is the only place
//! mc-net pushes a resident order, a movement goal or committed damage into the
//! regional entity owner. A resident order therefore never fabricates state:
//! each push is fenced on the same entity snapshot the owner reports.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;

use mc_entity::{
    EntityId, EntityItemStack, EntityLifecycle, EntitySnapshot, GoalState, Rotation, Vec3,
};
use mc_script::ScriptHostileCategory;
use uuid::Uuid;

use super::entity_combat::{
    begin_server_entity_death_locked, entity_kill_rewards_locked,
    publish_accepted_entity_health_locked,
};
use super::entity_lifecycle::nearby_entity_candidate_ids_locked;
use super::outbound::{VisibilityDispatch, dispatch_visibility_commands};
use super::projectiles::spawn_resident_arrow_locked;
use super::visibility::entity_hurt_dispatches_locked;

use super::{ENTITY_DEATH_TICKS, SessionEntityGuards, SessionRegistry};
use mc_script::precommit::HookKind;

/// One resident order push target: the engine goal to set, keyed by identity.
pub(crate) struct ResidentGoal {
    pub(crate) uuid: Uuid,
    pub(crate) goal: GoalState,
}

/// One committed damage request against a resident-perceived target.
#[derive(Debug, Clone)]
pub(crate) struct ResidentAttack {
    pub(crate) uuid: Uuid,
    pub(crate) amount: f32,
    /// Snapshot the request is fenced on; an entity that moved or changed since
    /// the decision is not damaged.
    pub(crate) expected: EntitySnapshot,
}

/// One committed damage outcome.
#[derive(Debug, Clone)]
pub(crate) struct ResidentHit {
    pub(crate) damage: f32,
    pub(crate) killed: bool,
}

/// One entity already visible in local perception.
pub(crate) struct ResidentCandidate {
    pub(crate) uuid: Uuid,
    pub(crate) position: Vec3,
    pub(crate) category: ScriptHostileCategory,
    pub(crate) type_name: String,
    pub(crate) animal: Option<mc_entity::AnimalBreedingState>,
}

impl SessionRegistry {
    /// Identities of players with a live authenticated session. Server PvP rules
    /// apply on top of the plugin policy: a resident never targets a player that
    /// is not an online actor.
    pub(crate) fn resident_live_player_uuids(&self) -> BTreeSet<Uuid> {
        let inner = self.lock_inner("resident live players");
        inner
            .sessions
            .values()
            .filter(|session| !session.tx.is_closed())
            .map(|session| session.uuid)
            .collect()
    }

    /// Authoritative pose of one authenticated player actor, when it is live in
    /// the simulated dimension.
    pub(crate) fn resident_actor_entity(&self, actor_id: u64) -> Option<(EntityId, Vec3)> {
        let inner = self.lock_inner("resident actor entity");
        let session = inner.sessions.get(&actor_id)?;
        if session.tx.is_closed()
            || session.dimension != super::script_resident_endpoint::RESIDENT_DIMENSION
        {
            return None;
        }
        Some((
            EntityId(session.entity_id),
            Vec3::new(session.pose.x, session.pose.y, session.pose.z),
        ))
    }

    /// Push one engine goal per resident through the regional owner. Returns how
    /// many goals were accepted; an unloaded or dead resident accepts none.
    pub(crate) async fn apply_resident_goals(&self, goals: Vec<ResidentGoal>) -> usize {
        if goals.is_empty() {
            return 0;
        }
        let owner = self.entities.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            let uuids = goals.iter().map(|goal| goal.uuid).collect::<Vec<_>>();
            let snapshots = owner.entities_by_uuid(&uuids)?;
            let pushes = goals
                .iter()
                .zip(snapshots.iter())
                .filter_map(|(goal, snapshot)| {
                    let snapshot = snapshot.as_ref()?;
                    (snapshot.lifecycle == EntityLifecycle::Alive)
                        .then(|| (snapshot.id, goal.goal.clone()))
                })
                .collect::<Vec<_>>();
            owner.set_goals_deferred_journal(pushes)
        })
        .await;
        match result {
            Ok(Ok(applied)) => applied,
            _ => 0,
        }
    }

    /// Launch a single world-physical arrow only if both actors still match
    /// their server-observed snapshots. Impact damage belongs to the shared
    /// projectile kernel, never to a plugin tick or the order receipt.
    pub(crate) fn launch_resident_arrow(
        &self,
        attacker: &EntitySnapshot,
        target: &EntitySnapshot,
    ) -> bool {
        let arrow_type = self.hostile_arrow_entity_type_id.load(Ordering::Acquire);
        if arrow_type < 0 {
            return false;
        }
        let mut inner = self.lock_session_entities("launch resident arrow");
        if inner.entities.snapshot(attacker.id).as_ref() != Some(attacker)
            || inner.entities.snapshot(target.id).as_ref() != Some(target)
            || attacker.lifecycle != EntityLifecycle::Alive
            || target.lifecycle != EntityLifecycle::Alive
        {
            return false;
        }
        let eye = Vec3::new(
            attacker.position.x,
            attacker.position.y + 1.5,
            attacker.position.z,
        );
        let horizontal = (target.position.x - eye.x).hypot(target.position.z - eye.z);
        let aim = Vec3::new(
            target.position.x - eye.x,
            target.position.y + 0.6 - eye.y + horizontal * 0.2,
            target.position.z - eye.z,
        );
        let length = (aim.x * aim.x + aim.y * aim.y + aim.z * aim.z).sqrt();
        if !length.is_finite() || length <= f64::EPSILON {
            return false;
        }
        let direction = Vec3::new(aim.x / length, aim.y / length, aim.z / length);
        let speed = crate::play::SKELETON_ARROW_SPEED;
        let velocity = Vec3::new(
            direction.x * speed,
            direction.y * speed,
            direction.z * speed,
        );
        let position = Vec3::new(
            eye.x + direction.x * 0.7,
            eye.y + direction.y * 0.7,
            eye.z + direction.z * 0.7,
        );
        let yaw = velocity.z.atan2(velocity.x).to_degrees() as f32 - 90.0;
        let pitch = (-velocity.y)
            .atan2(velocity.x.hypot(velocity.z))
            .to_degrees() as f32;
        let (_, dispatches) = spawn_resident_arrow_locked(
            &mut inner,
            attacker.id,
            target.id,
            arrow_type,
            position,
            velocity,
            Rotation {
                yaw,
                pitch,
                head_yaw: yaw,
            },
        );
        drop(inner);
        dispatch_visibility_commands(dispatches);
        true
    }
    /// Commit one bounded damage volley through the engine's own damage path.
    /// Each request is fenced on the snapshot it was decided from, so a stale
    /// or forged target commits nothing.
    pub(crate) async fn commit_resident_damage(
        &self,
        plugin_id: &str,
        attacks: Vec<ResidentAttack>,
    ) -> Vec<Option<ResidentHit>> {
        if attacks.is_empty() || plugin_id.is_empty() {
            return Vec::new();
        }
        if self
            .precommit_boundary()
            .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
        {
            let Some(handle) = self.damage_precommit_handle().cloned() else {
                return attacks.into_iter().map(|_| None).collect();
            };
            let mut hits = Vec::with_capacity(attacks.len());
            for attack in attacks.iter().cloned() {
                hits.push(
                    handle
                        .damage_resident_entity(plugin_id, attack)
                        .await
                        .ok()
                        .flatten(),
                );
            }
            return hits;
        }
        let tick = self.simulation_tick();
        let owner = self.entities.handle.clone();
        let ids = attacks
            .iter()
            .map(|attack| {
                (
                    attack.uuid,
                    attack.expected.id,
                    attack.expected.health.max(0.0),
                )
            })
            .collect::<Vec<_>>();
        let requests = attacks
            .into_iter()
            .map(|attack| {
                (
                    attack.expected,
                    mc_entity::EntityDamageRequest {
                        amount: attack.amount,
                        tick,
                        death_remove_tick: tick.saturating_add(ENTITY_DEATH_TICKS),
                        villager_gossip_event: None,
                    },
                )
            })
            .collect::<Vec<_>>();
        let committed =
            tokio::task::spawn_blocking(move || owner.damage_batch_if_current(requests)).await;
        let Ok(Ok(damages)) = committed else {
            return ids.iter().map(|_| None).collect();
        };
        let mut by_id = damages
            .into_iter()
            .map(|damage| (damage.snapshot.id, damage))
            .collect::<BTreeMap<_, _>>();
        let mut hits = Vec::with_capacity(ids.len());
        let mut committed = Vec::with_capacity(ids.len());
        for (_, id, before) in &ids {
            let Some(damage) = by_id.remove(id) else {
                hits.push(None);
                continue;
            };
            hits.push(Some(ResidentHit {
                damage: (*before - damage.snapshot.health).max(0.0),
                killed: damage.killed,
            }));
            committed.push(damage);
        }
        // The native batch already committed off-thread; the accepted state
        // must still reach the same health, hurt/death and reward publication
        // every other damage source uses.
        let mut inner = self.lock_session_entities("publish resident damage");
        let mut dispatches = publish_committed_resident_damage_locked(&mut inner, &committed);
        drop(inner);
        self.append_spawned_xp_pickup_candidates(&mut dispatches);
        dispatch_visibility_commands(dispatches);
        hits
    }

    /// Bounded local perception: entities already tracked in chunks around
    /// `center`, classified by the engine's own mob categories. Never scans the
    /// world; a resident only ever sees what the session already simulates.
    pub(crate) fn resident_perception(
        &self,
        center: Vec3,
        radius: f64,
        categories: &[ScriptHostileCategory],
    ) -> Vec<ResidentCandidate> {
        if categories.is_empty() || !center.is_finite() || !radius.is_finite() || radius <= 0.0 {
            return Vec::new();
        }
        let behaviors = self.mob_behavior_table();
        let inner = self.lock_session_entities("resident perception");
        let radius_sq = radius * radius;
        let live_players = inner
            .sessions
            .values()
            .filter(|session| !session.tx.is_closed())
            .map(|session| session.uuid)
            .collect::<BTreeSet<_>>();
        // Perception is the session's own tracked entity set, never a world
        // scan: the chunk index already carries exactly what a client sees.
        nearby_entity_candidate_ids_locked(&inner, center, radius)
            .into_iter()
            .filter_map(|id| inner.entities.snapshot(id))
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .filter(|entity| {
                let dx = entity.position.x - center.x;
                let dy = entity.position.y - center.y;
                let dz = entity.position.z - center.z;
                dx * dx + dy * dy + dz * dz <= radius_sq
            })
            .filter_map(|entity| {
                let category = resident_category(
                    inner.hostile_entities.contains(&entity.id),
                    &behaviors,
                    &entity.type_name,
                )?;
                if category == ScriptHostileCategory::Player && !live_players.contains(&entity.uuid)
                {
                    return None;
                }
                categories.contains(&category).then_some(ResidentCandidate {
                    uuid: entity.uuid,
                    position: entity.position,
                    category,
                    type_name: entity.type_name,
                    animal: entity.animal,
                })
            })
            .collect()
    }

    /// Project one resident's canonical main-hand stack onto the entity's held
    /// item. A resident with no entity in the owner accepts no projection.
    pub(crate) async fn set_resident_held_item(
        &self,
        uuid: Uuid,
        stack: Option<EntityItemStack>,
    ) -> bool {
        let owner = self.entities.handle.clone();
        tokio::task::spawn_blocking(move || {
            let Ok(snapshots) = owner.entities_by_uuid(&[uuid]) else {
                return false;
            };
            let Some(Some(snapshot)) = snapshots.into_iter().next() else {
                return false;
            };
            owner.set_item_stack(snapshot.id, stack).unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    }
}

/// Test-only tracked spawn: a hostile spawned here is visible to resident
/// perception through the same chunk index the server maintains.
#[cfg(test)]
impl SessionRegistry {
    pub(crate) fn spawn_tracked_entity_for_test(
        &self,
        entity: mc_entity::SpawnEntity,
        hostile: bool,
    ) -> Option<EntityId> {
        let position = entity.position;
        let mut inner = self.lock_session_entities("resident order test spawn");
        let id = inner.entities.spawn(entity);
        super::entity_lifecycle::track_entity_chunk_locked(&mut inner, id, position);
        if hostile {
            inner.hostile_entities.insert(id);
        }
        Some(id)
    }

    /// Test-only registration of an already-spawned entity in the same chunk
    /// index the server maintains, so an entity spawned through a non-session
    /// owner path (a materialised resident) participates in perception exactly
    /// as a session-spawned one does.
    pub(crate) fn track_entity_chunk_for_test(&self, id: EntityId) -> bool {
        let mut inner = self.lock_session_entities("resident order test track");
        let Some(position) = inner
            .entities
            .snapshot(id)
            .map(|snapshot| snapshot.position)
        else {
            return false;
        };
        super::entity_lifecycle::track_entity_chunk_locked(&mut inner, id, position);
        true
    }

    pub(crate) fn snapshot_entity_for_test(&self, id: EntityId) -> Option<EntitySnapshot> {
        let inner = self.lock_session_entities("resident order test snapshot");
        inner.entities.snapshot(id)
    }

    pub(crate) fn remove_tracked_entity_for_test(&self, id: EntityId) -> bool {
        let mut inner = self.lock_session_entities("resident order test remove");
        super::entity_lifecycle::remove_server_entity_state_locked(&mut inner, id).is_some()
    }
    pub(crate) fn resident_arrows_for_test(&self, center: Vec3) -> Vec<EntitySnapshot> {
        let inner = self.lock_session_entities("inspect resident arrows");
        nearby_entity_candidate_ids_locked(&inner, center, 16.0)
            .into_iter()
            .filter_map(|id| inner.entities.snapshot(id))
            .filter(|entity| {
                entity
                    .retained
                    .arrow_state
                    .is_some_and(|arrow| arrow.restricted_target.is_some())
            })
            .collect()
    }
}

fn resident_category(
    server_hostile: bool,
    behaviors: &mc_data::mob_behavior_26_1_2::MobBehaviorTable,
    type_name: &str,
) -> Option<ScriptHostileCategory> {
    if type_name == "minecraft:player" {
        return Some(ScriptHostileCategory::Player);
    }
    // Items, arrows and other non-living entities are never targets.
    if matches!(
        type_name,
        "minecraft:item"
            | "minecraft:arrow"
            | "minecraft:spectral_arrow"
            | "minecraft:experience_orb"
            | "minecraft:armor_stand"
    ) {
        return None;
    }
    // The server's own hostile tracking is authoritative; the AI policy and the
    // static fact table are the fallbacks for entities it has not classified.
    if server_hostile || crate::play::survival::is_hostile_entity(type_name) {
        return Some(ScriptHostileCategory::Hostile);
    }
    match behaviors.get_by_name(type_name) {
        // A mob with an attack policy is hostile; everything else that the
        // server simulates as a living entity is a neutral animal.
        Some(profile) if profile.combat != mc_data::mob_behavior_26_1_2::MobCombatPolicy::None => {
            Some(ScriptHostileCategory::Hostile)
        }
        _ => Some(ScriptHostileCategory::NeutralAnimal),
    }
}

/// Publish one committed off-thread resident damage batch into the session
/// projection: the committed health plus the hurt, death and reward dispatches
/// every other damage source uses.
///
/// The melee batch commits on the region owner lane while the session lock is
/// released, so the victim's motion, goal and ticking timers can advance before
/// this runs. The live snapshot therefore replaces the committed one only while
/// it still carries that exact damage; an entity whose damage state was changed
/// underneath belongs to that newer update.
pub(in crate::play::session) fn publish_committed_resident_damage_locked(
    inner: &mut SessionEntityGuards<'_>,
    committed: &[mc_entity::EntityDamage],
) -> Vec<VisibilityDispatch> {
    let mut dispatches = Vec::new();
    for damage in committed {
        let stamped = inner
            .entities
            .snapshot(damage.snapshot.id)
            .filter(|current| carries_committed_resident_damage(current, &damage.snapshot));
        let rewards = damage.killed.then(|| {
            entity_kill_rewards_locked(inner, stamped.as_ref().unwrap_or(&damage.snapshot))
        });
        let health = stamped.as_ref().map_or_else(Vec::new, |stamped| {
            publish_accepted_entity_health_locked(inner, stamped)
        });
        if let Some(rewards) = rewards {
            let (_, mut death) = begin_server_entity_death_locked(inner, damage, &rewards);
            death.splice(0..0, health);
            dispatches.append(&mut death);
        } else {
            let mut hurt = health;
            hurt.extend(entity_hurt_dispatches_locked(inner, damage.snapshot.id));
            dispatches.append(&mut hurt);
        }
    }
    dispatches
}

/// Whether `current` still carries the damage this off-thread batch committed.
/// Only the fields that mutation owns have to match: motion, goals, pathing and
/// ticking timers legitimately advance between the commit and the publication,
/// while a newer hit or a paid heal changes one of these fields and owns the
/// published health from then on.
fn carries_committed_resident_damage(current: &EntitySnapshot, committed: &EntitySnapshot) -> bool {
    current.id == committed.id
        && current.uuid == committed.uuid
        && current.health == committed.health
        && current.lifecycle == committed.lifecycle
        && current.retained.last_damage_tick == committed.retained.last_damage_tick
        && current.retained.death_remove_tick == committed.retained.death_remove_tick
        && current.retained.treatment_decision_id == committed.retained.treatment_decision_id
}
