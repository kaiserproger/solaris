use crate::effects_26_1_2::{
    ActiveEffects, CallerOwnedResult, EffectAction, EffectApplication, EffectDamageSource,
    EffectFlags, EffectId, EffectInstance, EffectKind, PendingEffectTick, TargetEffectContext,
    TickCommitError, TickPlanError, TickScratch, TickScratchCapacities, TickScratchError,
};
use crate::living_26_1_2::{
    DamageApplied, DamageContext, DamageEvent, DamageFlags, DamageOutcome, DamageRejection,
    DamageSource, DamageSourceKind, InvulnerabilityClock, LifecycleTransition, LivingLifecycle,
    LivingState, StateError, TickApplied, TickOutcome, apply_damage, tick_living,
};

use super::state::{RemovalReason, RuntimeState, StateRevision};

pub const MAX_DAMAGE_INPUTS_PER_TICK: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedDamage {
    pub event: DamageEvent,
    pub context: DamageContext,
}

#[derive(Debug, Clone, Copy)]
pub struct TickInput<'a> {
    pub entity_tick_count: i32,
    pub target_effect_context: TargetEffectContext,
    pub target_kind: TargetKind,
    pub mode: TickMode,
    /// Caller-selected vanilla-oracle order; every active ID must appear once.
    pub effect_action_order: &'a [EffectId],
    pub max_health: f32,
    pub invulnerability_clock: InvulnerabilityClock,
    pub should_tick_death: bool,
    pub damage_inputs: &'a [ResolvedDamage],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Player,
    NonPlayer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickMode {
    Normal,
    Kill,
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageOrigin {
    Queued { index: usize },
    Effect(EffectId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeDamageSource {
    Resolved(DamageSource),
    Effect(EffectDamageSource),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PublicationFact {
    DamageApplied {
        origin: DamageOrigin,
        source: RuntimeDamageSource,
        applied: DamageApplied,
    },
    DamageRejected {
        origin: DamageOrigin,
        source: RuntimeDamageSource,
        rejection: DamageRejection,
    },
    HurtEvent {
        origin: DamageOrigin,
        source: RuntimeDamageSource,
    },
    DeathStarted {
        origin: DamageOrigin,
        source: RuntimeDamageSource,
    },
    HealthChanged {
        effect_id: EffectId,
        amount: f32,
        before: f32,
        after: f32,
    },
    ExternalEffectAction {
        effect_id: EffectId,
        action: EffectAction,
    },
    EffectHurtCallback {
        effect_id: EffectId,
        amplifier: u8,
        origin: DamageOrigin,
        source: RuntimeDamageSource,
        damage: f32,
    },
    EffectRestored {
        effect: EffectInstance,
        refresh_attributes: bool,
    },
    EffectPeriodicSync {
        effect: EffectInstance,
    },
    EffectRemoved {
        effect: EffectInstance,
    },
    DeathCompleted,
    EffectRemovalCallback {
        effect: EffectInstance,
        reason: RemovalReason,
    },
    EntityRemoved {
        reason: RemovalReason,
    },
    EntityDeathGameEvent,
}

#[derive(Debug, Clone, Copy)]
struct EffectDamageResolution {
    context: DamageContext,
}

#[derive(Debug, Clone, Copy)]
struct DamageCallback {
    origin: DamageOrigin,
    source: RuntimeDamageSource,
    damage: f32,
}

#[derive(Debug, Clone, Copy)]
struct StagedAction {
    facts: [Option<PublicationFact>; 3],
    damage_callback: Option<DamageCallback>,
}

impl StagedAction {
    const NONE: Self = Self {
        facts: [None; 3],
        damage_callback: None,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct PreparedEffectAction {
    id: EffectId,
    current: EffectInstance,
    application: EffectApplication,
    damage_source: Option<EffectDamageSource>,
    damage_resolution: Option<EffectDamageResolution>,
    staged_action: StagedAction,
}

impl PreparedEffectAction {
    #[must_use]
    pub const fn id(&self) -> EffectId {
        self.id
    }

    #[must_use]
    pub const fn application(&self) -> EffectApplication {
        self.application
    }

    #[must_use]
    pub const fn damage_source(&self) -> Option<EffectDamageSource> {
        self.damage_source
    }

    pub fn resolve_damage(&mut self, context: DamageContext) -> Result<(), EffectResolutionError> {
        if !matches!(
            self.application,
            EffectApplication::Supported(
                EffectAction::MagicDamageIfHealthAbove { .. } | EffectAction::Damage { .. }
            )
        ) {
            return Err(EffectResolutionError::NotDamageAction(self.id));
        }
        if self.damage_resolution.is_some() {
            return Err(EffectResolutionError::AlreadyResolved(self.id));
        }
        self.damage_resolution = Some(EffectDamageResolution { context });
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectResolutionError {
    NotDamageAction(EffectId),
    AlreadyResolved(EffectId),
}

#[derive(Debug)]
pub struct PreparedTick<'a> {
    actions: &'a mut [PreparedEffectAction],
    effect_ticks: &'a mut [PendingEffectTick],
    will_remove: bool,
}

impl<'a> PreparedTick<'a> {
    #[must_use]
    pub fn actions(&self) -> &[PreparedEffectAction] {
        self.actions
    }

    pub fn actions_mut(&mut self) -> &mut [PreparedEffectAction] {
        self.actions
    }

    pub fn effect_ticks_mut(&mut self) -> &mut [PendingEffectTick] {
        self.effect_ticks
    }

    #[must_use]
    pub const fn will_remove(&self) -> bool {
        self.will_remove
    }
}

#[derive(Debug)]
pub struct AppliedTick<'a> {
    publications: &'a [PublicationFact],
}

impl AppliedTick<'_> {
    #[must_use]
    pub const fn publications(&self) -> &[PublicationFact] {
        self.publications
    }
}

#[derive(Debug, Clone, Copy)]
struct PreparedState {
    expected_revision: StateRevision,
    living: LivingState,
    max_health: f32,
    target_kind: TargetKind,
    removal_reason: Option<RemovalReason>,
    death_completed: bool,
    direct_kill: bool,
}

#[derive(Debug, Clone, Copy)]
struct CallbackEffectState {
    id: EffectId,
    amplifier: u8,
    active: bool,
}

#[derive(Debug)]
pub struct RuntimeScratch {
    damage_capacity: usize,
    effect_scratch: TickScratch,
    planned_effects: Vec<PreparedEffectAction>,
    queued_damage: Vec<StagedAction>,
    callback_effects: Vec<CallbackEffectState>,
    publications: Vec<PublicationFact>,
    prepared: Option<PreparedState>,
}

impl RuntimeScratch {
    pub fn try_new(
        effect_capacity: usize,
        damage_capacity: usize,
    ) -> Result<Self, RuntimeScratchError> {
        if damage_capacity > MAX_DAMAGE_INPUTS_PER_TICK {
            return Err(RuntimeScratchError::TooManyDamageInputs);
        }
        let effect_scratch =
            TickScratch::try_new(effect_capacity).map_err(RuntimeScratchError::EffectScratch)?;
        // Each successful damage can call every active effect. Both dimensions
        // have already passed their hard caps before this arithmetic.
        let callback_capacity = (damage_capacity + effect_capacity) * effect_capacity;
        let publication_capacity =
            damage_capacity * 3 + effect_capacity * 6 + callback_capacity + 2;

        let mut planned_effects = Vec::new();
        planned_effects
            .try_reserve_exact(effect_capacity)
            .map_err(|_| RuntimeScratchError::AllocationFailed)?;
        let mut queued_damage = Vec::new();
        queued_damage
            .try_reserve_exact(damage_capacity)
            .map_err(|_| RuntimeScratchError::AllocationFailed)?;
        let mut callback_effects = Vec::new();
        callback_effects
            .try_reserve_exact(effect_capacity)
            .map_err(|_| RuntimeScratchError::AllocationFailed)?;
        let mut publications = Vec::new();
        publications
            .try_reserve_exact(publication_capacity)
            .map_err(|_| RuntimeScratchError::AllocationFailed)?;

        Ok(Self {
            damage_capacity,
            effect_scratch,
            planned_effects,
            queued_damage,
            callback_effects,
            publications,
            prepared: None,
        })
    }

    #[must_use]
    pub fn capacities(&self) -> RuntimeScratchCapacities {
        RuntimeScratchCapacities {
            effect_ticks: self.effect_scratch.capacities(),
            planned_effects: self.planned_effects.capacity(),
            queued_damage: self.queued_damage.capacity(),
            callback_effects: self.callback_effects.capacity(),
            publications: self.publications.capacity(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeScratchCapacities {
    pub effect_ticks: TickScratchCapacities,
    pub planned_effects: usize,
    pub queued_damage: usize,
    pub callback_effects: usize,
    pub publications: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeScratchError {
    TooManyDamageInputs,
    AllocationFailed,
    EffectScratch(TickScratchError),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrepareError {
    Removed(RemovalReason),
    InvalidMaxHealth,
    KillWithDamage,
    DiscardWithDamage,
    TooManyDamageInputs {
        needed: usize,
        capacity: usize,
    },
    InvalidDamage {
        origin: DamageOrigin,
        rejection: DamageRejection,
    },
    LivingTick(StateError),
    EffectPlan(TickPlanError),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ApplyError {
    NoPreparedTick,
    StaleState {
        expected: StateRevision,
        actual: StateRevision,
    },
    UnresolvedEffectDamage(EffectId),
    InvalidDamage {
        origin: DamageOrigin,
        rejection: DamageRejection,
    },
    EffectCommit(TickCommitError),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectActionApplyError {
    InvalidMaxHealth,
    UnresolvedDamageContext,
    InvalidDamage(DamageRejection),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppliedEffectAction {
    publications: Vec<PublicationFact>,
}

impl AppliedEffectAction {
    #[must_use]
    pub fn publications(&self) -> &[PublicationFact] {
        &self.publications
    }
}

pub fn apply_effect_action(
    state: &mut RuntimeState,
    effect_id: EffectId,
    action: EffectAction,
    max_health: f32,
    target_kind: TargetKind,
    damage_context: Option<DamageContext>,
) -> Result<AppliedEffectAction, EffectActionApplyError> {
    if !max_health.is_finite() || max_health <= 0.0 {
        return Err(EffectActionApplyError::InvalidMaxHealth);
    }
    let application = EffectApplication::Supported(action);
    let damage_source = planned_effect_damage_source(application);
    if damage_source.is_some() && damage_context.is_none() {
        return Err(EffectActionApplyError::UnresolvedDamageContext);
    }
    let prepared = PreparedEffectAction {
        id: effect_id,
        current: EffectInstance::new(
            effect_id,
            EffectKind::CallerOwned,
            1,
            0,
            EffectFlags::default(),
        ),
        application,
        damage_source,
        damage_resolution: damage_context.map(|context| EffectDamageResolution { context }),
        staged_action: StagedAction::NONE,
    };
    let mut living = state.living();
    let staged =
        stage_effect_action(&prepared, &mut living, max_health, target_kind).map_err(|error| {
            match error {
                ApplyError::UnresolvedEffectDamage(_) => {
                    EffectActionApplyError::UnresolvedDamageContext
                }
                ApplyError::InvalidDamage { rejection, .. } => {
                    EffectActionApplyError::InvalidDamage(rejection)
                }
                ApplyError::NoPreparedTick
                | ApplyError::StaleState { .. }
                | ApplyError::EffectCommit(_) => {
                    unreachable!("direct effect actions do not use prepared tick state")
                }
            }
        })?;
    state.commit(living, state.removal_reason());

    Ok(AppliedEffectAction {
        publications: staged.facts.into_iter().flatten().collect(),
    })
}

pub fn prepare_tick<'a>(
    state: &RuntimeState,
    effects: &ActiveEffects,
    input: TickInput<'_>,
    scratch: &'a mut RuntimeScratch,
) -> Result<PreparedTick<'a>, PrepareError> {
    scratch.prepared = None;
    scratch.planned_effects.clear();
    scratch.queued_damage.clear();
    scratch.callback_effects.clear();
    scratch.publications.clear();

    if let Some(reason) = state.removal_reason() {
        return Err(PrepareError::Removed(reason));
    }
    if !input.damage_inputs.is_empty() {
        match input.mode {
            TickMode::Kill => return Err(PrepareError::KillWithDamage),
            TickMode::Discard => return Err(PrepareError::DiscardWithDamage),
            TickMode::Normal => {}
        }
    }
    if input.mode == TickMode::Normal && (!input.max_health.is_finite() || input.max_health <= 0.0)
    {
        return Err(PrepareError::InvalidMaxHealth);
    }
    if input.damage_inputs.len() > scratch.damage_capacity {
        return Err(PrepareError::TooManyDamageInputs {
            needed: input.damage_inputs.len(),
            capacity: scratch.damage_capacity,
        });
    }

    let mut living = state.living();
    let (removal_reason, death_completed) = match input.mode {
        TickMode::Kill => (Some(RemovalReason::Killed), false),
        TickMode::Discard => (Some(RemovalReason::Discarded), false),
        TickMode::Normal => {
            for (index, damage) in input.damage_inputs.iter().copied().enumerate() {
                let staged = stage_damage(
                    &mut living,
                    damage.event,
                    damage.context,
                    DamageOrigin::Queued { index },
                    RuntimeDamageSource::Resolved(damage.event.source),
                )
                .map_err(|rejection| PrepareError::InvalidDamage {
                    origin: DamageOrigin::Queued { index },
                    rejection,
                })?;
                scratch.queued_damage.push(staged);
            }

            advance_living_phase(
                &mut living,
                input.invulnerability_clock,
                input.should_tick_death,
            )?;
            let removed = living.lifecycle == LivingLifecycle::Removed;
            (removed.then_some(RemovalReason::Killed), removed)
        }
    };
    scratch.prepared = Some(PreparedState {
        expected_revision: state.revision(),
        living,
        max_health: input.max_health,
        target_kind: input.target_kind,
        removal_reason,
        death_completed,
        direct_kill: input.mode == TickMode::Kill,
    });

    let (effect_scratch, planned_effects) =
        (&mut scratch.effect_scratch, &mut scratch.planned_effects);
    let effect_ticks = match effects.plan_tick_batch(
        input.entity_tick_count,
        input.target_effect_context,
        input.effect_action_order,
        effect_scratch,
    ) {
        Ok(effect_ticks) => effect_ticks,
        Err(error) => {
            scratch.prepared = None;
            return Err(PrepareError::EffectPlan(error));
        }
    };
    let will_remove = removal_reason.is_some();
    for effect_tick in effect_ticks.iter_mut() {
        let id = effect_tick.id();
        planned_effects.push(PreparedEffectAction {
            id,
            current: effects
                .get(id)
                .expect("effect tick plans are sourced from active rows"),
            application: effect_tick.application(),
            damage_source: planned_effect_damage_source(effect_tick.application()),
            damage_resolution: None,
            staged_action: StagedAction::NONE,
        });
        if will_remove
            && matches!(
                effect_tick.application(),
                EffectApplication::CallerOwned { .. }
            )
        {
            effect_tick
                .resolve_caller_owned(CallerOwnedResult::Skipped)
                .expect("caller-owned application accepts a skipped removal result");
        }
    }

    Ok(PreparedTick {
        actions: planned_effects,
        effect_ticks,
        will_remove,
    })
}

pub fn apply_tick<'a>(
    state: &mut RuntimeState,
    effects: &mut ActiveEffects,
    scratch: &'a mut RuntimeScratch,
) -> Result<AppliedTick<'a>, ApplyError> {
    let prepared = scratch.prepared.ok_or(ApplyError::NoPreparedTick)?;
    if state.revision() != prepared.expected_revision {
        return Err(ApplyError::StaleState {
            expected: prepared.expected_revision,
            actual: state.revision(),
        });
    }

    let mut living = prepared.living;
    if prepared.removal_reason.is_none() {
        for effect in &mut scratch.planned_effects {
            effect.staged_action = stage_effect_action(
                effect,
                &mut living,
                prepared.max_health,
                prepared.target_kind,
            )?;
        }
    }

    let RuntimeScratch {
        effect_scratch,
        planned_effects,
        queued_damage,
        callback_effects,
        publications,
        prepared: prepared_slot,
        ..
    } = scratch;

    if let Some(reason) = prepared.removal_reason {
        // The effect batch is committed only to enforce its epoch fence. The
        // removal facts below use the pre-tick snapshots and the final store is
        // cleared, so no intermediate duration change is externally visible.
        effects
            .commit_tick_batch(effect_scratch)
            .map_err(ApplyError::EffectCommit)?;

        reset_callback_effects(callback_effects, planned_effects);
        publications.clear();
        for staged in queued_damage.iter().copied() {
            publish_staged_action(staged, callback_effects, publications);
        }
        if prepared.death_completed {
            publications.push(PublicationFact::DeathCompleted);
        }
        if reason.triggers_effect_removal_callbacks() {
            for effect in planned_effects.iter() {
                effects.remove(effect.id);
                publications.push(PublicationFact::EffectRemovalCallback {
                    effect: effect.current,
                    reason,
                });
            }
        }
        publications.push(PublicationFact::EntityRemoved { reason });
        if prepared.direct_kill {
            publications.push(PublicationFact::EntityDeathGameEvent);
        }
        state.commit(living, Some(reason));
        *prepared_slot = None;
        return Ok(AppliedTick { publications });
    }

    let outcomes = effects
        .commit_tick_batch(effect_scratch)
        .map_err(ApplyError::EffectCommit)?;
    reset_callback_effects(callback_effects, planned_effects);
    publications.clear();
    for staged in queued_damage.iter().copied() {
        publish_staged_action(staged, callback_effects, publications);
    }
    for (effect_index, (effect, outcome)) in planned_effects.iter().zip(outcomes).enumerate() {
        publish_staged_action(effect.staged_action, callback_effects, publications);
        if let Some(restored) = outcome.restored {
            publications.push(PublicationFact::EffectRestored {
                effect: restored,
                refresh_attributes: outcome.refresh_attributes,
            });
        }
        if let Some(periodic_sync) = outcome.periodic_sync {
            publications.push(PublicationFact::EffectPeriodicSync {
                effect: periodic_sync,
            });
        }
        if let Some(removed) = outcome.removed {
            publications.push(PublicationFact::EffectRemoved { effect: removed });
        }
        if let Some(restored) = outcome.restored {
            callback_effects[effect_index].amplifier = restored.amplifier;
        }
        if outcome.removed.is_some() {
            callback_effects[effect_index].active = false;
        }
    }

    state.commit(living, None);
    *prepared_slot = None;
    Ok(AppliedTick { publications })
}

fn stage_effect_action(
    effect: &PreparedEffectAction,
    living: &mut LivingState,
    max_health: f32,
    target_kind: TargetKind,
) -> Result<StagedAction, ApplyError> {
    let action = match effect.application {
        EffectApplication::None | EffectApplication::CallerOwned { .. } => {
            return Ok(StagedAction::NONE);
        }
        EffectApplication::Supported(action) => action,
    };

    match action {
        EffectAction::HealIfBelowMax { amount } => {
            if living.health < max_health {
                Ok(stage_heal(effect.id, living, max_health, amount))
            } else {
                Ok(StagedAction::NONE)
            }
        }
        EffectAction::Heal { amount } => Ok(stage_heal(effect.id, living, max_health, amount)),
        EffectAction::MagicDamageIfHealthAbove {
            amount,
            minimum_health,
        } => {
            let resolution = effect
                .damage_resolution
                .ok_or(ApplyError::UnresolvedEffectDamage(effect.id))?;
            if living.health > minimum_health {
                stage_effect_damage(effect, living, amount, resolution)
            } else {
                Ok(StagedAction::NONE)
            }
        }
        EffectAction::Damage { amount, .. } => {
            let resolution = effect
                .damage_resolution
                .ok_or(ApplyError::UnresolvedEffectDamage(effect.id))?;
            stage_effect_damage(effect, living, amount, resolution)
        }
        EffectAction::ExhaustPlayer { .. } | EffectAction::FeedPlayer { .. }
            if target_kind == TargetKind::Player =>
        {
            Ok(StagedAction {
                facts: [
                    Some(PublicationFact::ExternalEffectAction {
                        effect_id: effect.id,
                        action,
                    }),
                    None,
                    None,
                ],
                damage_callback: None,
            })
        }
        EffectAction::ExhaustPlayer { .. } | EffectAction::FeedPlayer { .. } => {
            Ok(StagedAction::NONE)
        }
    }
}

fn stage_heal(
    effect_id: EffectId,
    living: &mut LivingState,
    max_health: f32,
    amount: f32,
) -> StagedAction {
    if living.health <= 0.0 {
        return StagedAction::NONE;
    }
    let before = living.health;
    let after = (before + amount).clamp(0.0, max_health);
    living.health = after;
    if before == after {
        return StagedAction::NONE;
    }
    StagedAction {
        facts: [
            Some(PublicationFact::HealthChanged {
                effect_id,
                amount,
                before,
                after,
            }),
            None,
            None,
        ],
        damage_callback: None,
    }
}

fn stage_effect_damage(
    effect: &PreparedEffectAction,
    living: &mut LivingState,
    amount: f32,
    resolution: EffectDamageResolution,
) -> Result<StagedAction, ApplyError> {
    let source = effect
        .damage_source
        .expect("damage applications always bind an effect source during prepare");
    let origin = DamageOrigin::Effect(effect.id);
    stage_damage(
        living,
        DamageEvent {
            source: living_effect_damage_source(source),
            amount,
        },
        resolution.context,
        origin,
        RuntimeDamageSource::Effect(source),
    )
    .map_err(|rejection| ApplyError::InvalidDamage { origin, rejection })
}

fn stage_damage(
    living: &mut LivingState,
    event: DamageEvent,
    context: DamageContext,
    origin: DamageOrigin,
    source: RuntimeDamageSource,
) -> Result<StagedAction, DamageRejection> {
    let mut next = *living;
    let mut outcome = DamageOutcome::Rejected(DamageRejection::Dead);
    apply_damage(&mut next, event, context, &mut outcome);
    match outcome {
        DamageOutcome::Applied(applied) => {
            *living = next;
            Ok(StagedAction {
                facts: [
                    Some(PublicationFact::DamageApplied {
                        origin,
                        source,
                        applied,
                    }),
                    applied
                        .fresh_hurt
                        .then_some(PublicationFact::HurtEvent { origin, source }),
                    (applied.lifecycle == LifecycleTransition::StartedDying)
                        .then_some(PublicationFact::DeathStarted { origin, source }),
                ],
                damage_callback: Some(DamageCallback {
                    origin,
                    source,
                    damage: applied.raw_amount,
                }),
            })
        }
        DamageOutcome::Rejected(rejection) if is_invalid_damage_input(rejection) => Err(rejection),
        DamageOutcome::Rejected(rejection) => Ok(StagedAction {
            facts: [
                Some(PublicationFact::DamageRejected {
                    origin,
                    source,
                    rejection,
                }),
                None,
                None,
            ],
            damage_callback: None,
        }),
    }
}

fn advance_living_phase(
    living: &mut LivingState,
    invulnerability_clock: InvulnerabilityClock,
    should_tick_death: bool,
) -> Result<(), PrepareError> {
    if should_tick_death || living.lifecycle != LivingLifecycle::Dying {
        let mut outcome = TickOutcome::Advanced(TickApplied::Stable);
        tick_living(living, invulnerability_clock, &mut outcome);
        return match outcome {
            TickOutcome::Advanced(_) => Ok(()),
            TickOutcome::InvalidState(error) => Err(PrepareError::LivingTick(error)),
        };
    }

    living.validate().map_err(PrepareError::LivingTick)?;
    living.hurt_time = living.hurt_time.saturating_sub(1);
    if invulnerability_clock == InvulnerabilityClock::Kernel {
        living.invulnerable_time = living.invulnerable_time.saturating_sub(1);
    }
    Ok(())
}

const fn planned_effect_damage_source(
    application: EffectApplication,
) -> Option<EffectDamageSource> {
    match application {
        EffectApplication::Supported(EffectAction::MagicDamageIfHealthAbove { .. }) => {
            Some(EffectDamageSource::Magic)
        }
        EffectApplication::Supported(EffectAction::Damage { source, .. }) => Some(source),
        _ => None,
    }
}

const fn living_effect_damage_source(source: EffectDamageSource) -> DamageSource {
    // The living child kernel models resolved tags, but has no magic/wither
    // identity variants. RuntimeDamageSource retains the exact registry source
    // for callbacks and publications while this carrier supplies its tags.
    let flags = match source {
        EffectDamageSource::Magic | EffectDamageSource::Wither => {
            DamageFlags::BYPASSES_ARMOR.union(DamageFlags::NO_KNOCKBACK)
        }
        EffectDamageSource::IndirectMagic => DamageFlags::BYPASSES_ARMOR,
    };
    DamageSource::with_flags(DamageSourceKind::Generic, flags)
}

fn reset_callback_effects(
    callback_effects: &mut Vec<CallbackEffectState>,
    planned_effects: &[PreparedEffectAction],
) {
    callback_effects.clear();
    callback_effects.extend(planned_effects.iter().map(|effect| CallbackEffectState {
        id: effect.id,
        amplifier: effect.current.amplifier,
        active: true,
    }));
}

fn publish_staged_action(
    staged: StagedAction,
    callback_effects: &[CallbackEffectState],
    publications: &mut Vec<PublicationFact>,
) {
    publications.extend(staged.facts.into_iter().flatten());
    let Some(callback) = staged.damage_callback else {
        return;
    };
    for effect in callback_effects.iter().filter(|effect| effect.active) {
        publications.push(PublicationFact::EffectHurtCallback {
            effect_id: effect.id,
            amplifier: effect.amplifier,
            origin: callback.origin,
            source: callback.source,
            damage: callback.damage,
        });
    }
}

const fn is_invalid_damage_input(rejection: DamageRejection) -> bool {
    matches!(
        rejection,
        DamageRejection::InvalidState(_)
            | DamageRejection::NonFinite(_)
            | DamageRejection::OutOfRange(_)
            | DamageRejection::Unsupported(_)
    )
}
