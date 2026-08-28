use mc_entity::effects_26_1_2::{
    ActiveEffects, AddOutcome, CallerOwnedResult, EffectAction, EffectFlags, EffectId,
    EffectInstance, EffectKind, EffectLimits, TargetEffectContext, TickScratch,
};

use crate::lock_policy::lock_authoritative_mutex;
use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};

use super::outbound::{OutboundCommand, VisibilityDispatch};
use super::visibility::ordered_session_recipient;
use super::{SessionId, SessionRegistry, SessionRegistryInner};

pub(super) const SLOWNESS_EFFECT_ID: u32 = 2;
pub(super) const WEAKNESS_EFFECT_ID: u32 = 18;
pub(super) const POISON_EFFECT_ID: u32 = 19;
pub(super) const LEVITATION_EFFECT_ID: u32 = 24;

const PLAYER_EFFECT_CAPACITY: usize = 16;
const PLAYER_EFFECT_HIDDEN_CAPACITY: usize = 64;

#[derive(Debug)]
pub(super) struct PlayerEffectsState {
    effects: ActiveEffects,
    action_order: Vec<EffectId>,
    scratch: TickScratch,
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
            .filter_map(|outcome| outcome.periodic_sync)
            .collect::<Vec<_>>();
        self.action_order
            .retain(|effect_id| self.effects.get(*effect_id).is_some());
        PlayerEffectTick {
            actions,
            periodic_sync,
        }
    }

    fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

#[derive(Debug, Default)]
struct PlayerEffectTick {
    actions: Vec<EffectAction>,
    periodic_sync: Vec<EffectInstance>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlayerEffectFacts {
    pub health: f32,
    pub has_slowness: bool,
    pub has_poison: bool,
    pub has_weakness: bool,
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
    if inner.dead_sessions.contains(&session_id) {
        return Vec::new();
    }
    let Some(session) = inner.sessions.get(&session_id) else {
        return Vec::new();
    };
    let entity_id = session.entity_id;
    let recipient = ordered_session_recipient(session_id, session);
    let state = inner
        .player_effects
        .entry(session_id)
        .or_insert_with(PlayerEffectsState::new);
    let Some(current) = state.add(effect) else {
        return Vec::new();
    };
    vec![VisibilityDispatch {
        recipient,
        command: OutboundCommand::ApplyPlayerEffect {
            entity_id,
            effect_id: i32::try_from(current.id.raw()).unwrap_or(i32::MAX),
            amplifier: i32::from(current.amplifier),
            duration_ticks: current.duration,
        },
    }]
}

impl SessionRegistry {
    pub(super) fn player_effect_facts(&self, session_id: SessionId) -> Option<PlayerEffectFacts> {
        let inner = self.lock_inner("snapshot player effects for hostile targeting");
        let player_state = inner.player_persistence.get(&session_id)?.clone();
        let health = lock_authoritative_mutex(&player_state, "play.player_persistence")
            .survival
            .health;
        let effects = inner.player_effects.get(&session_id);
        Some(PlayerEffectFacts {
            health,
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
        let mut inner = self.lock_inner("tick player active effects");
        let mut session_ids = inner.player_effects.keys().copied().collect::<Vec<_>>();
        session_ids.sort_unstable();
        let mut dispatches = Vec::new();
        for session_id in session_ids {
            let Some(mut state) = inner.player_effects.remove(&session_id) else {
                continue;
            };
            if inner.dead_sessions.contains(&session_id)
                || !inner.sessions.contains_key(&session_id)
            {
                continue;
            }
            let health = inner
                .player_persistence
                .get(&session_id)
                .map(|player_state| {
                    lock_authoritative_mutex(player_state, "play.player_persistence")
                        .survival
                        .health
                });
            let tick_outcome = state.tick(tick);
            if let Some(session) = inner.sessions.get(&session_id) {
                let recipient = ordered_session_recipient(session_id, session);
                for effect in tick_outcome.periodic_sync {
                    dispatches.push(VisibilityDispatch {
                        recipient: recipient.clone(),
                        command: OutboundCommand::ApplyPlayerEffect {
                            entity_id: session.entity_id,
                            effect_id: i32::try_from(effect.id.raw()).unwrap_or(i32::MAX),
                            amplifier: i32::from(effect.amplifier),
                            duration_ticks: effect.duration,
                        },
                    });
                }
                for action in tick_outcome.actions {
                    if let EffectAction::MagicDamageIfHealthAbove {
                        amount,
                        minimum_health,
                    } = action
                        && health.is_some_and(|health| health > minimum_health)
                    {
                        dispatches.push(VisibilityDispatch {
                            recipient: recipient.clone(),
                            command: OutboundCommand::DamagePlayer {
                                damage: PlayerDamageRequest {
                                    kind: PlayerDamageKind::IndirectMagic,
                                    amount,
                                    source_origin: None,
                                },
                            },
                        });
                    }
                }
            }
            if !state.is_empty() {
                inner.player_effects.insert(session_id, state);
            }
        }
        dispatches
    }
}
