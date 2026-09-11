use mc_data::mob_effects_26_1_2::MobEffect;
use mc_entity::effects_26_1_2::{
    ActiveEffects, AddOutcome, CallerOwnedResult, EffectAction, EffectFlags, EffectId,
    EffectInstance, EffectKind, EffectLimits, TargetEffectContext, TickScratch,
};

use crate::lock_policy::lock_authoritative_mutex;
use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};

use super::outbound::{OutboundCommand, VisibilityDispatch};
use super::visibility::ordered_session_recipient;
use super::{SessionId, SessionRegistry, SessionRegistryInner};

pub(super) const SLOWNESS_EFFECT_ID: u32 = mc_data::mob_effects_26_1_2::MobEffect::Slowness as u32;
pub(super) const WEAKNESS_EFFECT_ID: u32 = mc_data::mob_effects_26_1_2::MobEffect::Weakness as u32;
pub(super) const POISON_EFFECT_ID: u32 = mc_data::mob_effects_26_1_2::MobEffect::Poison as u32;
pub(super) const LEVITATION_EFFECT_ID: u32 =
    mc_data::mob_effects_26_1_2::MobEffect::Levitation as u32;

const PLAYER_EFFECT_CAPACITY: usize = 16;
const PLAYER_EFFECT_HIDDEN_CAPACITY: usize = 64;

#[derive(Debug)]
pub(in crate::play) struct PlayerEffectsState {
    effects: ActiveEffects,
    action_order: Vec<EffectId>,
    scratch: TickScratch,
}

impl Clone for PlayerEffectsState {
    fn clone(&self) -> Self {
        Self::from_snapshot(self.snapshot()).expect("live effect store has a valid snapshot")
    }
}

impl PlayerEffectsState {
    fn new() -> Self {
        let limits = EffectLimits::new(PLAYER_EFFECT_CAPACITY, PLAYER_EFFECT_HIDDEN_CAPACITY)
            .expect("static player effect limits are within the hard cap");
        Self {
            effects: ActiveEffects::try_new(limits)
                .expect("bounded player effect store allocation must succeed"),
            action_order: Vec::with_capacity(PLAYER_EFFECT_CAPACITY),
            scratch: TickScratch::try_new(PLAYER_EFFECT_CAPACITY)
                .expect("bounded player effect scratch allocation must succeed"),
        }
    }

    pub(in crate::play) fn snapshot(&self) -> mc_entity::EntityActiveEffectsState {
        mc_entity::EntityActiveEffectsState {
            effects: self.effects.snapshot(),
            action_order: self.action_order.clone(),
        }
    }

    pub(in crate::play) fn from_snapshot(
        snapshot: mc_entity::EntityActiveEffectsState,
    ) -> Result<Self, &'static str> {
        let limits = EffectLimits::new(PLAYER_EFFECT_CAPACITY, PLAYER_EFFECT_HIDDEN_CAPACITY)
            .expect("static player effect limits are within the hard cap");
        let effects = ActiveEffects::try_from_snapshot(limits, &snapshot.effects)
            .map_err(|_| "invalid active-effect chains")?;
        if snapshot.action_order.len() != snapshot.effects.chains.len()
            || snapshot.action_order.iter().enumerate().any(|(index, id)| {
                effects.get(*id).is_none() || snapshot.action_order[..index].contains(id)
            })
        {
            return Err("invalid active-effect action order");
        }
        Ok(Self {
            effects,
            action_order: snapshot.action_order,
            scratch: TickScratch::try_new(PLAYER_EFFECT_CAPACITY)
                .expect("bounded player effect scratch allocation must succeed"),
        })
    }

    fn current_effects(&self) -> impl Iterator<Item = EffectInstance> + '_ {
        self.action_order
            .iter()
            .filter_map(|id| self.effects.get(*id))
    }

    pub(super) fn has(&self, raw_effect_id: u32) -> bool {
        self.effects.get(EffectId::new(raw_effect_id)).is_some()
    }

    fn add(&mut self, effect: EffectInstance) -> Option<EffectInstance> {
        let already_present = self.effects.get(effect.id).is_some();
        let outcome = self.effects.add(effect).ok()?;
        if !already_present {
            self.action_order.push(effect.id);
        }
        match outcome {
            AddOutcome::Added { current, .. } | AddOutcome::Updated { current, .. } => {
                Some(current)
            }
            AddOutcome::HiddenOnly { .. } | AddOutcome::Unchanged { .. } => None,
        }
    }

    fn tick(&mut self, tick: u64) -> PlayerEffectTick {
        let mut actions = Vec::new();
        let entity_tick_count = i32::try_from(tick).unwrap_or(i32::MAX);
        let Ok(pending) = self.effects.plan_tick_batch(
            entity_tick_count,
            TargetEffectContext::LIVING,
            &self.action_order,
            &mut self.scratch,
        ) else {
            return PlayerEffectTick::default();
        };
        for pending in pending {
            match pending.application() {
                mc_entity::effects_26_1_2::EffectApplication::Supported(action) => {
                    actions.push(action);
                }
                mc_entity::effects_26_1_2::EffectApplication::CallerOwned { .. } => {
                    let _ = pending.resolve_caller_owned(CallerOwnedResult::Continue);
                }
                mc_entity::effects_26_1_2::EffectApplication::None => {}
            }
        }
        let Ok(outcomes) = self.effects.commit_tick_batch(&mut self.scratch) else {
            return PlayerEffectTick::default();
        };
        let periodic_sync = outcomes
            .iter()
            .filter_map(|outcome| outcome.restored.or(outcome.periodic_sync))
            .collect::<Vec<_>>();
        let removed = outcomes
            .iter()
            .filter_map(|outcome| outcome.removed.map(|effect| effect.id))
            .collect();
        self.action_order
            .retain(|effect_id| self.effects.get(*effect_id).is_some());
        PlayerEffectTick {
            actions,
            periodic_sync,
            removed,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

#[derive(Debug, Default)]
struct PlayerEffectTick {
    actions: Vec<EffectAction>,
    periodic_sync: Vec<EffectInstance>,
    removed: Vec<EffectId>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlayerEffectFacts {
    pub health: f32,
    pub has_slowness: bool,
    pub has_poison: bool,
    pub has_weakness: bool,
}

pub(in crate::play) fn effect_kind(effect: MobEffect) -> EffectKind {
    match effect {
        MobEffect::Regeneration => EffectKind::Regeneration,
        MobEffect::Poison => EffectKind::Poison,
        MobEffect::Wither => EffectKind::Wither,
        MobEffect::Hunger => EffectKind::Hunger,
        MobEffect::Saturation => EffectKind::Saturation,
        MobEffect::InstantHealth => EffectKind::InstantHealth,
        MobEffect::InstantDamage => EffectKind::InstantDamage,
        _ => EffectKind::CallerOwned,
    }
}

fn effect_damage_request(action: EffectAction, health: f32) -> Option<PlayerDamageRequest> {
    let (kind, amount) = match action {
        EffectAction::MagicDamageIfHealthAbove {
            amount,
            minimum_health,
        } if health > minimum_health => (PlayerDamageKind::Magic, amount),
        EffectAction::Damage { amount, source } => {
            let kind = match source {
                mc_entity::effects_26_1_2::EffectDamageSource::Magic => PlayerDamageKind::Magic,
                mc_entity::effects_26_1_2::EffectDamageSource::IndirectMagic => {
                    PlayerDamageKind::IndirectMagic
                }
                mc_entity::effects_26_1_2::EffectDamageSource::Wither => PlayerDamageKind::Wither,
            };
            (kind, amount)
        }
        _ => return None,
    };
    Some(PlayerDamageRequest {
        kind,
        amount,
        source_origin: None,
    })
}

pub(super) fn caller_owned_effect(
    raw_effect_id: u32,
    duration_ticks: i32,
    amplifier: i32,
) -> EffectInstance {
    EffectInstance::new(
        EffectId::new(raw_effect_id),
        EffectKind::CallerOwned,
        duration_ticks,
        amplifier,
        EffectFlags::default(),
    )
}

pub(super) fn poison_effect(duration_ticks: i32, amplifier: i32) -> EffectInstance {
    EffectInstance::new(
        EffectId::new(POISON_EFFECT_ID),
        EffectKind::Poison,
        duration_ticks,
        amplifier,
        EffectFlags::default(),
    )
}

pub(super) fn apply_player_effect_locked(
    inner: &mut SessionRegistryInner,
    session_id: SessionId,
    effect: EffectInstance,
) -> Vec<VisibilityDispatch> {
    let Some(shared) = inner.player_persistence.get(&session_id).cloned() else {
        return Vec::new();
    };
    let mut player = lock_authoritative_mutex(&shared, "play.player_persistence");
    apply_player_effect_to_state_locked(inner, session_id, &mut player, effect)
}

pub(super) fn apply_player_effect_to_state_locked(
    inner: &mut SessionRegistryInner,
    session_id: SessionId,
    player: &mut crate::play::persistence::PlayerPersistedState,
    effect: EffectInstance,
) -> Vec<VisibilityDispatch> {
    if player.survival.is_dead() || !inner.sessions.contains_key(&session_id) {
        return Vec::new();
    }
    let state = player.effects.get_or_insert_with(PlayerEffectsState::new);
    let current = state.add(effect);
    if !state.is_empty() {
        inner.player_effect_sessions.insert(session_id);
    }
    current.map_or_else(Vec::new, |current| {
        player_effect_publication_locked(inner, session_id, current)
    })
}

fn player_effect_publication_locked(
    inner: &SessionRegistryInner,
    session_id: SessionId,
    effect: EffectInstance,
) -> Vec<VisibilityDispatch> {
    super::player_state_adapter::player_publication_including_self_locked(
        inner,
        session_id,
        |entity_id| player_effect_command(entity_id, effect),
    )
}

pub(in crate::play) fn player_effect_command(
    entity_id: i32,
    effect: EffectInstance,
) -> OutboundCommand {
    OutboundCommand::ApplyPlayerEffect {
        entity_id,
        effect_id: i32::try_from(effect.id.raw()).expect("canonical effect id fits VarInt"),
        amplifier: i32::from(effect.amplifier),
        duration_ticks: effect.duration,
        flags: effect.flags,
    }
}

fn player_effect_removal_locked(
    inner: &SessionRegistryInner,
    session_id: SessionId,
    effect_id: EffectId,
) -> Vec<VisibilityDispatch> {
    super::player_state_adapter::player_publication_including_self_locked(
        inner,
        session_id,
        |entity_id| OutboundCommand::RemovePlayerEffect {
            entity_id,
            effect_id: i32::try_from(effect_id.raw()).expect("canonical effect id fits VarInt"),
        },
    )
}

fn apply_player_resource_effect_locked(
    inner: &SessionRegistryInner,
    session_id: SessionId,
    action: EffectAction,
) -> Option<VisibilityDispatch> {
    let session = inner.sessions.get(&session_id)?;
    let shared = inner.player_persistence.get(&session_id)?;
    let mut player = lock_authoritative_mutex(shared, "play.player_persistence");
    let before = player.survival;
    if before.is_dead() {
        return None;
    }
    match action {
        EffectAction::Heal { amount } | EffectAction::HealIfBelowMax { amount } => {
            player.survival.heal(amount);
        }
        EffectAction::ExhaustPlayer { amount } => {
            if matches!(
                player.game_mode,
                mc_domain::GameMode::Creative | mc_domain::GameMode::Spectator
            ) {
                return None;
            }
            player.survival.add_exhaustion(amount);
        }
        EffectAction::FeedPlayer {
            food,
            saturation_modifier,
        } => {
            player
                .survival
                .add_food(food, food as f32 * 2.0 * saturation_modifier);
        }
        EffectAction::MagicDamageIfHealthAbove { .. } | EffectAction::Damage { .. } => {
            return None;
        }
    }
    (player.survival != before).then(|| VisibilityDispatch {
        recipient: ordered_session_recipient(session_id, session),
        command: OutboundCommand::PlayerSurvivalChanged {
            survival: player.survival,
        },
    })
}

impl SessionRegistry {
    pub(in crate::play) fn player_effect_snapshot(
        &self,
        session_id: SessionId,
    ) -> Vec<EffectInstance> {
        let inner = self.lock_inner("snapshot visible player effects");
        let Some(shared) = inner.player_persistence.get(&session_id) else {
            return Vec::new();
        };
        let player = lock_authoritative_mutex(shared, "play.player_persistence");
        player
            .effects
            .as_ref()
            .map_or_else(Vec::new, |effects| effects.current_effects().collect())
    }

    pub(in crate::play) fn broadcast_player_effects(
        &self,
        session_id: SessionId,
    ) -> Vec<VisibilityDispatch> {
        let inner = self.lock_inner("publish restored player effects");
        let Some(shared) = inner.player_persistence.get(&session_id) else {
            return Vec::new();
        };
        let player = lock_authoritative_mutex(shared, "play.player_persistence");
        let Some(effects) = player.effects.as_ref() else {
            return Vec::new();
        };
        effects
            .current_effects()
            .flat_map(|effect| player_effect_publication_locked(&inner, session_id, effect))
            .collect()
    }

    pub(super) fn player_effect_facts(&self, session_id: SessionId) -> Option<PlayerEffectFacts> {
        let inner = self.lock_inner("snapshot player effects for hostile targeting");
        let shared = inner.player_persistence.get(&session_id)?;
        let player = lock_authoritative_mutex(shared, "play.player_persistence");
        let effects = player.effects.as_ref();
        Some(PlayerEffectFacts {
            health: player.survival.health,
            has_slowness: effects.is_some_and(|effects| effects.has(SLOWNESS_EFFECT_ID)),
            has_poison: effects.is_some_and(|effects| effects.has(POISON_EFFECT_ID)),
            has_weakness: effects.is_some_and(|effects| effects.has(WEAKNESS_EFFECT_ID)),
        })
    }

    pub(in crate::play) fn tick_player_effects_owned(
        &self,
        _authority: &crate::play::simulation::SimulationAuthority,
        tick: u64,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("tick player active effects");
        let mut session_ids = inner
            .player_effect_sessions
            .iter()
            .copied()
            .collect::<Vec<_>>();
        session_ids.sort_unstable();
        let mut dispatches = Vec::new();
        for session_id in session_ids {
            if !inner.sessions.contains_key(&session_id) {
                inner.player_effect_sessions.remove(&session_id);
                continue;
            }
            let Some(shared) = inner.player_persistence.get(&session_id).cloned() else {
                inner.player_effect_sessions.remove(&session_id);
                continue;
            };
            let tick_outcome = {
                let mut player = lock_authoritative_mutex(&shared, "play.player_persistence");
                if player.survival.is_dead() {
                    player.effects = None;
                    inner.player_effect_sessions.remove(&session_id);
                    continue;
                }
                let Some(state) = player.effects.as_mut() else {
                    inner.player_effect_sessions.remove(&session_id);
                    continue;
                };
                let outcome = state.tick(tick);
                if state.is_empty() {
                    player.effects = None;
                    inner.player_effect_sessions.remove(&session_id);
                }
                outcome
            };
            for action in tick_outcome.actions {
                if let Some(publication) =
                    apply_player_resource_effect_locked(&inner, session_id, action)
                {
                    dispatches.push(publication);
                }
                let health = lock_authoritative_mutex(&shared, "play.player_persistence")
                    .survival
                    .health;
                if let Some(damage) = effect_damage_request(action, health) {
                    let preview = super::player_combat::prepare_projectile_player_damage_locked(
                        &inner, session_id, tick, damage,
                    );
                    match preview {
                        super::player_combat::ProjectilePlayerDamagePreview::Accepted(prepared)
                        | super::player_combat::ProjectilePlayerDamagePreview::Rejected(Some(
                            prepared,
                        )) => {
                            super::player_combat::commit_projectile_player_damage_locked(
                                &mut inner,
                                prepared,
                                |_| true,
                                &mut dispatches,
                            );
                        }
                        super::player_combat::ProjectilePlayerDamagePreview::Rejected(None) => {}
                    }
                }
            }
            for effect in tick_outcome.periodic_sync {
                dispatches.extend(player_effect_publication_locked(&inner, session_id, effect));
            }
            for effect_id in tick_outcome.removed {
                dispatches.extend(player_effect_removal_locked(&inner, session_id, effect_id));
            }
        }
        let became_no_live_sessions = self.publish_live_session_count(&inner);
        drop(inner);
        if became_no_live_sessions {
            self.reconcile_hostile_targets_after_live_session_change();
        }
        self.append_spawned_xp_pickup_candidates(&mut dispatches);
        dispatches
    }
}
