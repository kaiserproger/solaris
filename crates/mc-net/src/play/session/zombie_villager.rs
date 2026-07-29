use std::time::Instant;

use mc_entity::zombie_villager_26_1_2::{
    conversion_duration_from_seed, finish_conversion, start_conversion,
};
use mc_entity::{AttributeKind, EntityLifecycle, EntitySnapshot, SpawnEntity};
use mc_protocol::packets::play::{GameMode, ItemStack};
use tracing::warn;

use crate::play::inventory::PlayerInventory;
use crate::play::simulation::{
    CommittedZombieVillagerCure, SimulationAuthority, ZombieVillagerCurePlan,
};

use super::entity_lifecycle::clear_removed_entity_tracking_locked;
use super::interaction_geometry::{entity_geometry, within_entity_reach};
use super::visibility::{
    despawn_entity_visibility_locked, entity_event_dispatches_locked,
    initialize_entity_wire_state_from_snapshot_locked, server_entity_snapshot_from,
    spawn_entity_visibility_from_snapshot_locked,
};
use super::{
    SessionEntityGuards, SessionId, SessionRegistry, SessionRegistryInner,
    record_entity_dispatches_locked,
};

pub(super) const ZOMBIE_VILLAGER_CONVERSIONS_PER_TICK: usize = 4;
const VILLAGER_TYPE_ID_26_1_2: i32 = 139;
const ZOMBIE_VILLAGER_CURE_EVENT: i8 = 16;

impl SessionRegistry {
    pub(in crate::play) fn commit_zombie_villager_cure(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        plan: &ZombieVillagerCurePlan,
    ) -> Option<CommittedZombieVillagerCure> {
        let mut inner = self.lock_session_entities("commit zombie villager cure");
        let session = inner.sessions.get(&actor_session)?;
        if session.tx.is_closed() || inner.dead_sessions.contains(&actor_session) {
            return None;
        }
        let player_pose = session.pose;
        let player_uuid = session.uuid;
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let guard = player_state.lock().unwrap_or_else(|poisoned| {
            warn!(
                session_id = actor_session,
                "player persistence mutex was poisoned during zombie villager cure; recovering state"
            );
            poisoned.into_inner()
        });
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit zombie villager cure",
            wait_started,
            guard,
        );
        if player_state.game_mode == GameMode::Spectator || player_state.survival.is_dead() {
            return None;
        }
        let selected_slot =
            PlayerInventory::HOTBAR_BASE + usize::from(player_state.selected_hotbar_slot);
        if plan.held_slot != selected_slot && plan.held_slot != PlayerInventory::OFFHAND_SLOT {
            return None;
        }
        if player_state.inventory.slots[plan.held_slot] != plan.expected_held
            || plan.expected_held.is_empty()
            || plan.expected_held.item_id != plan.golden_apple_item_id
        {
            return None;
        }

        let expected = inner.entities.snapshot(plan.entity_id)?;
        if expected.lifecycle != EntityLifecycle::Alive
            || expected.type_name != "minecraft:zombie_villager"
            || !within_entity_reach(
                player_pose,
                expected.position,
                entity_geometry(&expected.type_name, expected.animal).aabb,
                player_state.game_mode,
            )
            || !inner
                .published_entity_snapshots
                .contains_key(&plan.entity_id)
        {
            return None;
        }

        let current_tick = inner.entity_lifecycle_tick;
        let duration =
            conversion_duration_from_seed(conversion_seed(&expected, player_uuid, current_tick));
        let next = start_conversion(&expected, Some(player_uuid), current_tick, duration).ok()?;
        let mut inventory = player_state.inventory.clone();
        let mut changed_slots = Vec::new();
        if player_state.game_mode != GameMode::Creative {
            let held = &mut inventory.slots[plan.held_slot];
            if held.count == 1 {
                *held = ItemStack::EMPTY;
            } else {
                held.count -= 1;
            }
            changed_slots.push((plan.held_slot, held.clone()));
        }

        if !inner
            .entities
            .replace_snapshot_if_current(expected, next.clone())
        {
            return None;
        }
        inner
            .published_entity_snapshots
            .insert(plan.entity_id, server_entity_snapshot_from(next.clone()));
        schedule_zombie_villager_conversion_locked(&mut inner, &next);
        player_state.replace_inventory(inventory.clone());

        let dispatches =
            entity_event_dispatches_locked(&inner, plan.entity_id, ZOMBIE_VILLAGER_CURE_EVENT);
        record_entity_dispatches_locked(&mut inner, &dispatches);
        Some(CommittedZombieVillagerCure {
            inventory,
            changed_slots,
            dispatches,
        })
    }
}

fn conversion_seed(entity: &EntitySnapshot, player: uuid::Uuid, current_tick: u64) -> u64 {
    let entity = entity.uuid.as_u128();
    let player = player.as_u128();
    (entity as u64)
        ^ ((entity >> 64) as u64)
        ^ (player as u64)
        ^ ((player >> 64) as u64)
        ^ current_tick
}

pub(super) fn schedule_zombie_villager_conversion_locked(
    inner: &mut SessionRegistryInner,
    entity: &EntitySnapshot,
) {
    let entity_id = entity.id;
    let Some(deadline) = entity
        .retained
        .zombie_villager_conversion
        .map(|conversion| conversion.completes_tick)
    else {
        inner
            .zombie_villager_conversion_deadline_by_id
            .remove(&entity_id);
        return;
    };
    if inner
        .zombie_villager_conversion_deadline_by_id
        .get(&entity_id)
        == Some(&deadline)
    {
        return;
    }
    if let Some(previous) = inner
        .zombie_villager_conversion_deadline_by_id
        .insert(entity_id, deadline)
    {
        let remove_bucket = inner
            .zombie_villager_conversion_deadlines
            .get_mut(&previous)
            .is_some_and(|bucket| {
                bucket.retain(|queued| *queued != entity_id);
                bucket.is_empty()
            });
        if remove_bucket {
            inner.zombie_villager_conversion_deadlines.remove(&previous);
        }
    }
    inner
        .zombie_villager_conversion_deadlines
        .entry(deadline)
        .or_default()
        .push_back(entity_id);
}

pub(super) fn finish_due_zombie_villager_conversions_locked(
    inner: &mut SessionEntityGuards<'_>,
    current_tick: u64,
) -> Vec<super::VisibilityDispatch> {
    let mut due_ids = Vec::with_capacity(ZOMBIE_VILLAGER_CONVERSIONS_PER_TICK);
    while due_ids.len() < ZOMBIE_VILLAGER_CONVERSIONS_PER_TICK {
        let Some((&deadline, _)) = inner.zombie_villager_conversion_deadlines.first_key_value()
        else {
            break;
        };
        if deadline > current_tick {
            break;
        }
        let queue = inner
            .zombie_villager_conversion_deadlines
            .get_mut(&deadline)
            .expect("first conversion deadline exists");
        let entity_id = queue
            .pop_front()
            .expect("conversion deadline queue is non-empty");
        if queue.is_empty() {
            inner.zombie_villager_conversion_deadlines.remove(&deadline);
        }
        if inner
            .zombie_villager_conversion_deadline_by_id
            .get(&entity_id)
            != Some(&deadline)
        {
            continue;
        }
        inner
            .zombie_villager_conversion_deadline_by_id
            .remove(&entity_id);
        due_ids.push(entity_id);
    }

    let mut dispatches = Vec::new();
    for entity_id in due_ids {
        let Some(expected) = inner.entities.snapshot(entity_id) else {
            continue;
        };
        let mut next = match finish_conversion(&expected, current_tick, VILLAGER_TYPE_ID_26_1_2) {
            Ok(Some(next)) => next,
            Ok(None) => {
                schedule_zombie_villager_conversion_locked(inner, &expected);
                continue;
            }
            Err(_) => continue,
        };
        let mut villager_facts =
            SpawnEntity::new(VILLAGER_TYPE_ID_26_1_2, "minecraft:villager", next.position);
        super::apply_entity_facts(&mut villager_facts);
        next.attributes = villager_facts.attributes;
        next.health = next
            .attributes
            .base(&AttributeKind::MaxHealth)
            .unwrap_or(20.0) as f32;
        let old_published = inner
            .published_entity_snapshots
            .get(&entity_id)
            .cloned()
            .unwrap_or_else(|| server_entity_snapshot_from(expected.clone()));
        if !inner
            .entities
            .convert_snapshot_if_current(expected, next.clone())
        {
            if let Some(current) = inner.entities.snapshot(entity_id) {
                schedule_zombie_villager_conversion_locked(inner, &current);
            }
            continue;
        }

        dispatches.extend(despawn_entity_visibility_locked(inner, &old_published));
        clear_removed_entity_tracking_locked(inner, entity_id);
        inner
            .entity_type_aabbs
            .entry(VILLAGER_TYPE_ID_26_1_2)
            .or_insert_with(|| super::interaction_geometry::entity_aabb("minecraft:villager"));
        super::entity_lifecycle::track_entity_chunk_locked(inner, entity_id, next.position);
        let next_published = server_entity_snapshot_from(next);
        initialize_entity_wire_state_from_snapshot_locked(inner, &next_published);
        dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
            inner,
            next_published,
        ));
    }
    dispatches
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use mc_entity::effects_26_1_2::{
        ActiveEffectChainSnapshot, ActiveEffectsSnapshot, EffectFlags, EffectInstance, EffectKind,
    };
    use mc_entity::zombie_villager_26_1_2::{
        NAUSEA_EFFECT_ID, STRENGTH_EFFECT_ID, WEAKNESS_EFFECT_ID,
    };
    use mc_entity::{EntityActiveEffectsState, EntityId, EntitySnapshot, Vec3};
    use tokio::sync::mpsc;

    use crate::login::{LoggedInProfile, offline_uuid};
    use crate::play::PlayerPose;
    use crate::play::persistence::PlayerPersistedState;

    use super::super::OutboundCommand;
    use super::*;

    const GOLDEN_APPLE_ITEM_ID: u32 = 733;

    fn register_player(
        registry: &SessionRegistry,
        name: &str,
        inventory: PlayerInventory,
        mode: GameMode,
    ) -> (
        SessionId,
        Arc<Mutex<PlayerPersistedState>>,
        mpsc::Receiver<OutboundCommand>,
    ) {
        let profile = LoggedInProfile {
            uuid: offline_uuid(name),
            name: name.to_owned(),
        };
        let (tx, rx) = mpsc::channel(32);
        let pose = PlayerPose::new(0.5, 64.0, 0.5);
        let session = registry
            .register(&profile, (0, 0), 2, HashSet::new(), tx, pose)
            .0;
        let mut state = PlayerPersistedState::new_default(pose);
        state.inventory = inventory;
        state.game_mode = mode;
        let state = Arc::new(Mutex::new(state));
        registry.register_player_persistence(session, Arc::clone(&state));
        (session, state, rx)
    }

    fn spawn_weak_zombie_villager(
        registry: &SessionRegistry,
        authority: &SimulationAuthority,
        position: Vec3,
    ) -> EntityId {
        registry.spawn_command_entity(
            authority,
            153,
            "minecraft:zombie_villager".to_owned(),
            position,
        );
        let mut inner = registry.lock_session_entities("seed weak zombie villager");
        let expected = inner
            .entities
            .snapshots()
            .max_by_key(|snapshot| snapshot.id)
            .expect("spawned zombie villager");
        let id = expected.id;
        let next = with_weakness(expected.clone());
        assert!(
            inner
                .entities
                .replace_snapshot_if_current(expected, next.clone())
        );
        inner
            .published_entity_snapshots
            .insert(id, server_entity_snapshot_from(next));
        id
    }

    fn with_weakness(mut snapshot: EntitySnapshot) -> EntitySnapshot {
        snapshot.retained.active_effects = Some(EntityActiveEffectsState {
            effects: ActiveEffectsSnapshot {
                chains: vec![ActiveEffectChainSnapshot {
                    current: EffectInstance::new(
                        WEAKNESS_EFFECT_ID,
                        EffectKind::CallerOwned,
                        600,
                        0,
                        EffectFlags::default(),
                    ),
                    hidden: Vec::new(),
                }],
            },
            action_order: vec![WEAKNESS_EFFECT_ID],
        });
        snapshot
    }

    fn with_preexisting_strength(mut snapshot: EntitySnapshot) -> EntitySnapshot {
        let effects = snapshot.retained.active_effects.as_mut().unwrap();
        effects.effects.chains.insert(
            0,
            ActiveEffectChainSnapshot {
                current: EffectInstance::new(
                    STRENGTH_EFFECT_ID,
                    EffectKind::CallerOwned,
                    200,
                    1,
                    EffectFlags::default(),
                ),
                hidden: Vec::new(),
            },
        );
        effects.action_order.insert(0, STRENGTH_EFFECT_ID);
        snapshot
            .attributes
            .set_base(AttributeKind::AttackDamage, 6.0);
        snapshot
    }

    fn cure_plan(entity_id: EntityId, expected_held: ItemStack) -> ZombieVillagerCurePlan {
        ZombieVillagerCurePlan {
            entity_id,
            held_slot: PlayerInventory::HOTBAR_BASE,
            expected_held,
            golden_apple_item_id: GOLDEN_APPLE_ITEM_ID,
        }
    }

    #[test]
    fn owner_cure_start_debits_apple_and_publishes_event_16() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(GOLDEN_APPLE_ITEM_ID, 2);
        let (session, player_state, _rx) = register_player(
            &registry,
            "CuringAlice",
            inventory.clone(),
            GameMode::Survival,
        );
        let entity_id =
            spawn_weak_zombie_villager(&registry, &authority, Vec3::new(1.5, 64.0, 0.5));
        {
            let mut inner = registry.lock_session_entities("make curing target visible");
            let visible = &mut inner
                .sessions
                .get_mut(&session)
                .expect("registered session")
                .visible_entities;
            visible.insert(entity_id);
            visible.publish();
        }

        let committed = registry
            .commit_zombie_villager_cure(
                &authority,
                session,
                &cure_plan(
                    entity_id,
                    inventory.slots[PlayerInventory::HOTBAR_BASE].clone(),
                ),
            )
            .expect("cure commits");

        assert_eq!(
            committed.inventory.slots[PlayerInventory::HOTBAR_BASE].count,
            1
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            committed.inventory.slots[PlayerInventory::HOTBAR_BASE]
        );
        assert_eq!(committed.changed_slots.len(), 1);
        assert!(committed.dispatches.iter().any(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::EntityEvent {
                    entity_id: id,
                    event_id: ZOMBIE_VILLAGER_CURE_EVENT,
                } if id == entity_id.0
            )
        }));
        let converting = registry
            .lock_entities("read converting zombie villager")
            .snapshot(entity_id)
            .expect("converting snapshot");
        let conversion = converting
            .retained
            .zombie_villager_conversion
            .expect("conversion retained");
        assert_eq!(conversion.started_by, Some(offline_uuid("CuringAlice")));
        assert!((3_600..=6_000).contains(&conversion.completes_tick));
        let effects = converting.retained.active_effects.expect("active effects");
        assert_eq!(effects.action_order, [STRENGTH_EFFECT_ID]);
    }

    #[test]
    fn lifecycle_converts_at_most_four_with_same_identity_and_ordered_visibility() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(GOLDEN_APPLE_ITEM_ID, 5);
        let (session, _, _rx) =
            register_player(&registry, "CuringBob", inventory, GameMode::Creative);
        let starter = offline_uuid("CuringBob");
        let mut identities = HashMap::new();
        for offset in 0..5 {
            let id = spawn_weak_zombie_villager(
                &registry,
                &authority,
                Vec3::new(1.5 + f64::from(offset), 64.0, 0.5),
            );
            let mut inner = registry.lock_session_entities("seed due conversion");
            let expected = inner.entities.snapshot(id).expect("zombie snapshot");
            identities.insert(id, expected.uuid);
            let next =
                start_conversion(&expected, Some(starter), 0, 3_600).expect("start conversion");
            assert!(
                inner
                    .entities
                    .replace_snapshot_if_current(expected, next.clone())
            );
            inner
                .published_entity_snapshots
                .insert(id, server_entity_snapshot_from(next.clone()));
            schedule_zombie_villager_conversion_locked(&mut inner, &next);
            let observer = inner
                .sessions
                .get_mut(&session)
                .expect("registered session");
            observer.loaded.insert((0, 0));
            observer.visible_entities.insert(id);
            observer.visible_entities.publish();
        }

        let dispatches = registry.tick_dying_entities(&authority, 3_600);
        let snapshots = registry
            .lock_entities("read converted villagers")
            .snapshots()
            .collect::<Vec<_>>();
        let villagers = snapshots
            .iter()
            .filter(|snapshot| snapshot.type_name == "minecraft:villager")
            .collect::<Vec<_>>();
        assert_eq!(villagers.len(), ZOMBIE_VILLAGER_CONVERSIONS_PER_TICK);
        assert_eq!(
            snapshots
                .iter()
                .filter(|snapshot| snapshot.type_name == "minecraft:zombie_villager")
                .count(),
            1
        );
        for villager in &villagers {
            assert_eq!(villager.type_id, VILLAGER_TYPE_ID_26_1_2);
            assert_eq!(identities[&villager.id], villager.uuid);
            assert!(villager.retained.zombie_villager_conversion.is_none());
            assert_eq!(
                villager
                    .retained
                    .active_effects
                    .as_ref()
                    .expect("nausea")
                    .action_order,
                [STRENGTH_EFFECT_ID, NAUSEA_EFFECT_ID]
            );
            assert_eq!(
                villager.attributes.base(&AttributeKind::MaxHealth),
                Some(20.0)
            );
            assert_eq!(
                villager
                    .retained
                    .villager_gossip
                    .as_ref()
                    .expect("cure gossip")
                    .player_reputation(starter),
                125
            );
        }
        let first = villagers[0].id;
        let ordered = dispatches
            .iter()
            .filter_map(|dispatch| match &dispatch.command {
                OutboundCommand::DespawnEntity(snapshot) if snapshot.id == first => Some("despawn"),
                OutboundCommand::SpawnEntity(snapshot) if snapshot.id == first => Some("spawn"),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ordered, ["despawn", "spawn"]);
        let inner = registry.lock_inner("read hostile tracking after cure");
        for villager in villagers {
            assert!(!inner.hostile_entities.contains(&villager.id));
            assert!(!inner.natural_hostile_mobs.contains(&villager.id));
        }
    }

    #[test]
    fn lifecycle_preserves_preexisting_strength_and_its_modifier() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let entity_id =
            spawn_weak_zombie_villager(&registry, &authority, Vec3::new(1.5, 64.0, 0.5));
        {
            let mut inner = registry.lock_session_entities("seed preexisting strength conversion");
            let expected = inner.entities.snapshot(entity_id).expect("zombie snapshot");
            let strengthened = with_preexisting_strength(expected.clone());
            assert!(
                inner
                    .entities
                    .replace_snapshot_if_current(expected, strengthened.clone())
            );
            let converting =
                start_conversion(&strengthened, Some(offline_uuid("StrongCurer")), 0, 3_600)
                    .expect("start conversion with preexisting strength");
            assert!(
                inner
                    .entities
                    .replace_snapshot_if_current(strengthened, converting.clone())
            );
            schedule_zombie_villager_conversion_locked(&mut inner, &converting);
        }

        registry.tick_dying_entities(&authority, 3_600);
        let cured = registry
            .lock_entities("read strong cured villager")
            .snapshot(entity_id)
            .expect("cured villager");
        assert_eq!(cured.type_name, "minecraft:villager");
        assert_eq!(
            cured
                .retained
                .active_effects
                .as_ref()
                .expect("preserved effects")
                .action_order,
            [STRENGTH_EFFECT_ID, NAUSEA_EFFECT_ID]
        );
        assert_eq!(
            cured.attributes.base(&AttributeKind::AttackDamage),
            Some(0.0),
            "Strength remains an effect; it must not become permanent base damage"
        );
    }

    #[test]
    fn malformed_persisted_conversion_is_rejected_without_partial_restore() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let entity_id =
            spawn_weak_zombie_villager(&registry, &authority, Vec3::new(1.5, 64.0, 0.5));
        {
            let mut inner = registry.lock_session_entities("seed invalid conversion fixtures");
            let expected = inner.entities.snapshot(entity_id).expect("zombie snapshot");
            let next = start_conversion(&expected, Some(offline_uuid("InvalidCurer")), 0, 3_600)
                .expect("start conversion");
            assert!(
                inner
                    .entities
                    .replace_snapshot_if_current(expected, next.clone())
            );
            schedule_zombie_villager_conversion_locked(&mut inner, &next);
        }
        let checkpoint = registry.persisted_entity_save_snapshot().0;
        let assert_rejected = |checkpoint| {
            let restored = SessionRegistry::new();
            assert_eq!(restored.restore_persisted_entities(checkpoint), 0);
            assert_eq!(
                restored
                    .lock_entities("verify invalid conversion restore stayed empty")
                    .snapshots()
                    .count(),
                0
            );
        };

        let mut wrong_type = checkpoint.clone();
        wrong_type.records[0].snapshot.type_id = 4;
        wrong_type.records[0].snapshot.type_name = "minecraft:cow".to_owned();
        assert_rejected(wrong_type);

        let mut dead = checkpoint.clone();
        dead.records[0].snapshot.lifecycle = EntityLifecycle::Despawning;
        dead.records[0].snapshot.retained.death_remove_tick = Some(20);
        assert_rejected(dead);

        let mut missing_strength = checkpoint.clone();
        missing_strength.records[0].snapshot.retained.active_effects = None;
        assert_rejected(missing_strength);

        let mut weakness_restored = checkpoint;
        let effects = weakness_restored.records[0]
            .snapshot
            .retained
            .active_effects
            .as_mut()
            .expect("conversion effects");
        effects.effects.chains.push(ActiveEffectChainSnapshot {
            current: EffectInstance::new(
                WEAKNESS_EFFECT_ID,
                EffectKind::CallerOwned,
                20,
                0,
                EffectFlags::default(),
            ),
            hidden: Vec::new(),
        });
        effects.action_order.push(WEAKNESS_EFFECT_ID);
        assert_rejected(weakness_restored);
    }

    #[test]
    fn persisted_conversion_resumes_at_the_same_deadline() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let entity_id =
            spawn_weak_zombie_villager(&registry, &authority, Vec3::new(1.5, 64.0, 0.5));
        let starter = offline_uuid("RestartCurer");
        {
            let mut inner = registry.lock_session_entities("seed persisted conversion");
            let expected = inner.entities.snapshot(entity_id).expect("zombie snapshot");
            let next =
                start_conversion(&expected, Some(starter), 0, 3_600).expect("start conversion");
            assert!(
                inner
                    .entities
                    .replace_snapshot_if_current(expected, next.clone())
            );
            inner
                .published_entity_snapshots
                .insert(entity_id, server_entity_snapshot_from(next.clone()));
            schedule_zombie_villager_conversion_locked(&mut inner, &next);
        }
        let checkpoint = registry.persisted_entity_save_snapshot().0;

        let restored = SessionRegistry::new();
        assert_eq!(restored.restore_persisted_entities(checkpoint), 1);
        assert!(restored.tick_dying_entities(&authority, 3_599).is_empty());
        let before = restored
            .lock_entities("read restored converting zombie")
            .snapshot(entity_id)
            .expect("restored zombie");
        assert_eq!(before.type_name, "minecraft:zombie_villager");
        restored.tick_dying_entities(&authority, 3_600);
        let cured = restored
            .lock_entities("read restored cured villager")
            .snapshot(entity_id)
            .expect("restored villager");
        assert_eq!(cured.type_name, "minecraft:villager");
        assert_eq!(cured.uuid, before.uuid);
        assert_eq!(
            cured
                .retained
                .villager_gossip
                .as_ref()
                .expect("persisted cure gossip")
                .player_reputation(starter),
            125
        );
    }
}
