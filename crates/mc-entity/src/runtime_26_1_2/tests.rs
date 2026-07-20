use super::*;
use crate::effects_26_1_2::{
    ActiveEffects, CallerOwnedResult, EffectDamageSource, EffectFlags, EffectKind, EffectLimits,
    MAX_ACTIVE_EFFECTS, TickCommitError, TickScratchError,
};
use crate::living_26_1_2::{
    DamageInputField, DamageRejection, DamageSourceKind, EnchantmentProtection, LivingLifecycle,
    StateError, UnsupportedRule,
};

const REGENERATION: EffectId = EffectId::new(10);
const POISON: EffectId = EffectId::new(11);
const WITHER: EffectId = EffectId::new(12);
const HUNGER: EffectId = EffectId::new(13);
const SATURATION: EffectId = EffectId::new(14);
const CALLER_OWNED: EffectId = EffectId::new(99);

fn effect(id: EffectId, kind: EffectKind, duration: i32, amplifier: i32) -> EffectInstance {
    EffectInstance::new(id, kind, duration, amplifier, EffectFlags::default())
}

fn effects(active: usize, hidden: usize) -> ActiveEffects {
    ActiveEffects::try_new(EffectLimits::new(active, hidden).unwrap()).unwrap()
}

#[test]
fn direct_effect_action_commits_heal_and_damage_through_runtime_state() {
    let mut state = alive(8.0);
    let healed = apply_effect_action(
        &mut state,
        EffectId::new(6),
        EffectAction::Heal { amount: 20.0 },
        20.0,
        TargetKind::NonPlayer,
        None,
    )
    .expect("instant health transaction");
    assert_eq!(state.living().health, 20.0);
    assert_eq!(
        healed.publications(),
        &[PublicationFact::HealthChanged {
            effect_id: EffectId::new(6),
            amount: 20.0,
            before: 8.0,
            after: 20.0,
        }]
    );

    let damaged = apply_effect_action(
        &mut state,
        EffectId::new(7),
        EffectAction::Damage {
            amount: 4.0,
            source: EffectDamageSource::Magic,
        },
        20.0,
        TargetKind::NonPlayer,
        Some(DamageContext::default()),
    )
    .expect("instant damage transaction");
    assert_eq!(state.living().health, 16.0);
    assert!(matches!(
        damaged.publications(),
        [PublicationFact::DamageApplied {
            origin: DamageOrigin::Effect(id),
            source: RuntimeDamageSource::Effect(EffectDamageSource::Magic),
            ..
        }, PublicationFact::HurtEvent { .. }]
            if *id == EffectId::new(7)
    ));
}

#[test]
fn direct_effect_action_requires_resolved_damage_context_without_mutation() {
    let mut state = alive(20.0);
    let before = state;
    assert_eq!(
        apply_effect_action(
            &mut state,
            EffectId::new(7),
            EffectAction::Damage {
                amount: 4.0,
                source: EffectDamageSource::Magic,
            },
            20.0,
            TargetKind::NonPlayer,
            None,
        ),
        Err(EffectActionApplyError::UnresolvedDamageContext)
    );
    assert_eq!(state, before);
}

fn alive(health: f32) -> RuntimeState {
    RuntimeState::try_new(LivingState::new(health, 0.0).unwrap(), None).unwrap()
}

fn generic_damage(amount: f32) -> ResolvedDamage {
    ResolvedDamage {
        event: DamageEvent {
            source: DamageSource::vanilla(DamageSourceKind::Generic),
            amount,
        },
        context: DamageContext::default(),
    }
}

fn input<'a>(damage_inputs: &'a [ResolvedDamage]) -> TickInput<'a> {
    TickInput {
        entity_tick_count: 200,
        target_effect_context: TargetEffectContext::LIVING,
        target_kind: TargetKind::Player,
        mode: TickMode::Normal,
        effect_action_order: &[],
        max_health: 20.0,
        invulnerability_clock: InvulnerabilityClock::Kernel,
        should_tick_death: true,
        damage_inputs,
    }
}

fn ordered_input<'a>(
    damage_inputs: &'a [ResolvedDamage],
    effect_action_order: &'a [EffectId],
) -> TickInput<'a> {
    TickInput {
        effect_action_order,
        ..input(damage_inputs)
    }
}

#[test]
fn queued_damage_then_timers_then_effects_publish_in_vanilla_phase_order() {
    let mut state = alive(10.0);
    let mut active = effects(2, 0);
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 50, 0))
        .unwrap();
    active
        .add(effect(POISON, EffectKind::Poison, 25, 0))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 1).unwrap();
    let queued = [generic_damage(1.0)];

    let order = [REGENERATION, POISON];
    let mut prepared = prepare_tick(
        &state,
        &active,
        ordered_input(&queued, &order),
        &mut scratch,
    )
    .unwrap();
    assert_eq!(
        prepared
            .actions()
            .iter()
            .map(PreparedEffectAction::id)
            .collect::<Vec<_>>(),
        vec![REGENERATION, POISON]
    );
    prepared.actions_mut()[1]
        .resolve_damage(DamageContext::default())
        .unwrap();

    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    let facts = applied.publications();
    assert!(matches!(
        facts[0],
        PublicationFact::DamageApplied {
            origin: DamageOrigin::Queued { index: 0 },
            ..
        }
    ));
    assert!(matches!(
        facts[1],
        PublicationFact::HurtEvent {
            origin: DamageOrigin::Queued { index: 0 },
            ..
        }
    ));
    assert_eq!(
        facts[2],
        PublicationFact::EffectHurtCallback {
            effect_id: REGENERATION,
            amplifier: 0,
            origin: DamageOrigin::Queued { index: 0 },
            source: RuntimeDamageSource::Resolved(
                DamageSource::vanilla(DamageSourceKind::Generic,)
            ),
            damage: 1.0,
        }
    );
    assert_eq!(
        facts[3],
        PublicationFact::EffectHurtCallback {
            effect_id: POISON,
            amplifier: 0,
            origin: DamageOrigin::Queued { index: 0 },
            source: RuntimeDamageSource::Resolved(
                DamageSource::vanilla(DamageSourceKind::Generic,)
            ),
            damage: 1.0,
        }
    );
    assert_eq!(
        facts[4],
        PublicationFact::HealthChanged {
            effect_id: REGENERATION,
            amount: 1.0,
            before: 9.0,
            after: 10.0,
        }
    );
    assert_eq!(
        facts[5],
        PublicationFact::DamageRejected {
            origin: DamageOrigin::Effect(POISON),
            source: RuntimeDamageSource::Effect(EffectDamageSource::Magic),
            rejection: DamageRejection::HurtCooldown,
        }
    );
    assert_eq!(facts.len(), 6);
    assert_eq!(state.living().health, 10.0);
    assert_eq!(state.living().hurt_time, 9);
    assert_eq!(state.living().invulnerable_time, 19);
}

#[test]
fn queued_lethal_damage_starts_death_before_advancing_its_clock() {
    let mut state = alive(1.0);
    let mut active = effects(0, 0);
    let mut scratch = RuntimeScratch::try_new(0, 1).unwrap();
    let queued = [generic_damage(2.0)];

    prepare_tick(&state, &active, input(&queued), &mut scratch).unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert!(matches!(
        applied.publications(),
        [
            PublicationFact::DamageApplied { .. },
            PublicationFact::HurtEvent { .. },
            PublicationFact::DeathStarted {
                origin: DamageOrigin::Queued { index: 0 },
                ..
            }
        ]
    ));
    assert_eq!(state.living().lifecycle, LivingLifecycle::Dying);
    assert_eq!(state.living().death_time, 1);
}

#[test]
fn death_tick_twenty_publishes_completion_effect_removal_callback_then_removal() {
    let living = LivingState {
        health: 0.0,
        absorption: 0.0,
        invulnerable_time: 3,
        hurt_time: 2,
        last_hurt: 4.0,
        lifecycle: LivingLifecycle::Dying,
        death_time: 19,
    };
    let mut state = RuntimeState::try_new(living, None).unwrap();
    let mut active = effects(1, 0);
    let current = effect(CALLER_OWNED, EffectKind::CallerOwned, 5, 2);
    active.add(current).unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();

    let order = [CALLER_OWNED];
    let prepared = prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    assert!(prepared.will_remove());
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();

    assert_eq!(
        applied.publications(),
        [
            PublicationFact::DeathCompleted,
            PublicationFact::EffectRemovalCallback {
                effect: current,
                reason: RemovalReason::Killed,
            },
            PublicationFact::EntityRemoved {
                reason: RemovalReason::Killed,
            },
        ]
    );
    assert_eq!(state.living().lifecycle, LivingLifecycle::Removed);
    assert_eq!(state.removal_reason(), Some(RemovalReason::Killed));
    assert!(active.is_empty());
}

#[test]
fn kernel_and_external_invulnerability_clocks_are_distinct() {
    for (clock, expected_invulnerable) in [
        (InvulnerabilityClock::Kernel, 4),
        (InvulnerabilityClock::External, 5),
    ] {
        let mut living = LivingState::new(10.0, 0.0).unwrap();
        living.invulnerable_time = 5;
        living.hurt_time = 3;
        let mut state = RuntimeState::try_new(living, None).unwrap();
        let mut active = effects(0, 0);
        let mut scratch = RuntimeScratch::try_new(0, 0).unwrap();
        let mut tick_input = input(&[]);
        tick_input.invulnerability_clock = clock;

        prepare_tick(&state, &active, tick_input, &mut scratch).unwrap();
        let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
        assert!(applied.publications().is_empty());
        assert_eq!(state.living().hurt_time, 2);
        assert_eq!(state.living().invulnerable_time, expected_invulnerable);
    }
}

#[test]
fn paused_death_clock_still_advances_hurt_and_invulnerability_timers() {
    let living = LivingState {
        health: 0.0,
        absorption: 0.0,
        invulnerable_time: 2,
        hurt_time: 2,
        last_hurt: 1.0,
        lifecycle: LivingLifecycle::Dying,
        death_time: 7,
    };
    let mut state = RuntimeState::try_new(living, None).unwrap();
    let mut active = effects(0, 0);
    let mut scratch = RuntimeScratch::try_new(0, 0).unwrap();
    let mut tick_input = input(&[]);
    tick_input.should_tick_death = false;

    prepare_tick(&state, &active, tick_input, &mut scratch).unwrap();
    apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert_eq!(state.living().death_time, 7);
    assert_eq!(state.living().hurt_time, 1);
    assert_eq!(state.living().invulnerable_time, 1);
}

#[test]
fn unresolved_caller_owned_effect_fails_without_any_mutation() {
    let mut state = alive(10.0);
    let before_state = state;
    let mut active = effects(1, 0);
    let current = effect(CALLER_OWNED, EffectKind::CallerOwned, 2, 0);
    active.add(current).unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();

    let order = [CALLER_OWNED];
    prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::EffectCommit(TickCommitError::UnresolvedCallerOwned(CALLER_OWNED))
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(CALLER_OWNED), Some(current));
}

#[test]
fn resolved_caller_owned_effect_commits_through_existing_effect_kernel() {
    let mut state = alive(10.0);
    let mut active = effects(1, 0);
    active
        .add(effect(CALLER_OWNED, EffectKind::CallerOwned, 2, 0))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();

    let order = [CALLER_OWNED];
    let mut prepared =
        prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    prepared.effect_ticks_mut()[0]
        .resolve_caller_owned(CallerOwnedResult::Continue)
        .unwrap();
    apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert_eq!(active.get(CALLER_OWNED).unwrap().duration, 1);
}

#[test]
fn unresolved_effect_damage_fails_atomically() {
    let mut state = alive(10.0);
    let before_state = state;
    let mut active = effects(1, 0);
    let current = effect(WITHER, EffectKind::Wither, 40, 0);
    active.add(current).unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();

    let order = [WITHER];
    prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::UnresolvedEffectDamage(WITHER)
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(WITHER), Some(current));
}

#[test]
fn stale_state_and_stale_effect_batches_are_rejected_atomically() {
    let mut state = alive(10.0);
    let mut active = effects(1, 0);
    let original = effect(REGENERATION, EffectKind::Regeneration, 50, 0);
    active.add(original).unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();

    let order = [REGENERATION];
    prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    let expected_revision = state.revision();
    state.replace_living(state.living()).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::StaleState {
            expected: expected_revision,
            actual: state.revision(),
        }
    );
    assert_eq!(active.get(REGENERATION), Some(original));

    prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    let before_state = state;
    let updated = effect(REGENERATION, EffectKind::Regeneration, 100, 0);
    active.add(updated).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::EffectCommit(TickCommitError::StalePlan)
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(REGENERATION), Some(updated));
}

#[test]
fn invalid_late_effect_damage_rolls_back_earlier_effect_actions() {
    let mut state = alive(10.0);
    let before_state = state;
    let mut active = effects(2, 0);
    let regeneration = effect(REGENERATION, EffectKind::Regeneration, 50, 0);
    let wither = effect(WITHER, EffectKind::Wither, 40, 0);
    active.add(regeneration).unwrap();
    active.add(wither).unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();

    let order = [REGENERATION, WITHER];
    let mut prepared =
        prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    let mut context = DamageContext::default();
    context.reductions.enchantment = EnchantmentProtection::UnsupportedSourceEvaluation;
    prepared.actions_mut()[1].resolve_damage(context).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::InvalidDamage {
            origin: DamageOrigin::Effect(WITHER),
            rejection: DamageRejection::Unsupported(UnsupportedRule::EnchantmentSourceEvaluation,),
        }
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(REGENERATION), Some(regeneration));
    assert_eq!(active.get(WITHER), Some(wither));
}

#[test]
fn invalid_queued_damage_and_invalid_tick_inputs_fail_during_prepare() {
    let state = alive(10.0);
    let active = effects(0, 0);
    let mut scratch = RuntimeScratch::try_new(0, 1).unwrap();
    let bad_damage = [ResolvedDamage {
        event: DamageEvent {
            source: DamageSource::vanilla(DamageSourceKind::Generic),
            amount: f32::NAN,
        },
        context: DamageContext::default(),
    }];
    assert_eq!(
        prepare_tick(&state, &active, input(&bad_damage), &mut scratch).unwrap_err(),
        PrepareError::InvalidDamage {
            origin: DamageOrigin::Queued { index: 0 },
            rejection: DamageRejection::NonFinite(DamageInputField::Amount),
        }
    );

    let unsupported = [ResolvedDamage {
        event: DamageEvent {
            source: DamageSource::vanilla(DamageSourceKind::Unsupported),
            amount: 1.0,
        },
        context: DamageContext::default(),
    }];
    assert_eq!(
        prepare_tick(&state, &active, input(&unsupported), &mut scratch).unwrap_err(),
        PrepareError::InvalidDamage {
            origin: DamageOrigin::Queued { index: 0 },
            rejection: DamageRejection::Unsupported(UnsupportedRule::DamageSource),
        }
    );

    let mut too_many = RuntimeScratch::try_new(0, 0).unwrap();
    assert_eq!(
        prepare_tick(
            &state,
            &active,
            input(&[generic_damage(1.0)]),
            &mut too_many
        )
        .unwrap_err(),
        PrepareError::TooManyDamageInputs {
            needed: 1,
            capacity: 0,
        }
    );

    let mut invalid_max = input(&[]);
    invalid_max.max_health = f32::INFINITY;
    assert_eq!(
        prepare_tick(&state, &active, invalid_max, &mut scratch).unwrap_err(),
        PrepareError::InvalidMaxHealth
    );
}

#[test]
fn effect_scratch_capacity_failure_is_typed_and_non_mutating() {
    let state = alive(10.0);
    let mut active = effects(2, 0);
    let first = effect(REGENERATION, EffectKind::Regeneration, 50, 0);
    let second = effect(POISON, EffectKind::Poison, 25, 0);
    active.add(first).unwrap();
    active.add(second).unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();

    assert_eq!(
        prepare_tick(
            &state,
            &active,
            ordered_input(&[], &[REGENERATION, POISON]),
            &mut scratch,
        )
        .unwrap_err(),
        PrepareError::EffectPlan(TickPlanError::ScratchTooSmall {
            needed: 2,
            capacity: 1,
        })
    );
    assert_eq!(active.get(REGENERATION), Some(first));
    assert_eq!(active.get(POISON), Some(second));
}

#[test]
fn invalid_effect_order_clears_the_runtime_transaction() {
    let mut state = alive(10.0);
    let before_state = state;
    let mut active = effects(1, 0);
    let current = effect(REGENERATION, EffectKind::Regeneration, 50, 0);
    active.add(current).unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();

    assert_eq!(
        prepare_tick(&state, &active, input(&[]), &mut scratch).unwrap_err(),
        PrepareError::EffectPlan(TickPlanError::OrderLengthMismatch {
            active: 1,
            provided: 0,
        })
    );
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::NoPreparedTick
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(REGENERATION), Some(current));
}

#[test]
fn effect_resolution_rejects_wrong_action_and_duplicate_input() {
    let state = alive(10.0);
    let mut active = effects(2, 0);
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 50, 0))
        .unwrap();
    active
        .add(effect(WITHER, EffectKind::Wither, 40, 0))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();

    let order = [REGENERATION, WITHER];
    let mut prepared =
        prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    assert_eq!(
        prepared.actions_mut()[0]
            .resolve_damage(DamageContext::default())
            .unwrap_err(),
        EffectResolutionError::NotDamageAction(REGENERATION)
    );
    prepared.actions_mut()[1]
        .resolve_damage(DamageContext::default())
        .unwrap();
    assert_eq!(
        prepared.actions_mut()[1]
            .resolve_damage(DamageContext::default())
            .unwrap_err(),
        EffectResolutionError::AlreadyResolved(WITHER)
    );
}

#[test]
fn effect_action_and_lifecycle_facts_are_interleaved_by_effect_id() {
    let mut state = alive(10.0);
    let mut active = effects(2, 1);
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 601, 0))
        .unwrap();
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 1, 1))
        .unwrap();
    let hunger = effect(HUNGER, EffectKind::Hunger, 1, 0);
    active.add(hunger).unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();

    let order = [REGENERATION, HUNGER];
    prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    let restored = effect(REGENERATION, EffectKind::Regeneration, 600, 0);
    let expired_hunger = effect(HUNGER, EffectKind::Hunger, 0, 0);
    assert_eq!(
        applied.publications(),
        [
            PublicationFact::EffectRestored {
                effect: restored,
                refresh_attributes: true,
            },
            PublicationFact::EffectPeriodicSync { effect: restored },
            PublicationFact::ExternalEffectAction {
                effect_id: HUNGER,
                action: EffectAction::ExhaustPlayer { amount: 0.005 },
            },
            PublicationFact::EffectRemoved {
                effect: expired_hunger,
            },
        ]
    );
}

#[test]
fn caller_selected_effect_order_controls_actions_and_publications() {
    let mut state = alive(10.0);
    let mut active = effects(2, 0);
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 50, 0))
        .unwrap();
    active
        .add(effect(HUNGER, EffectKind::Hunger, 2, 0))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();
    let order = [HUNGER, REGENERATION];

    let prepared = prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    assert_eq!(
        prepared
            .actions()
            .iter()
            .map(PreparedEffectAction::id)
            .collect::<Vec<_>>(),
        order
    );
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert_eq!(
        applied.publications(),
        [
            PublicationFact::ExternalEffectAction {
                effect_id: HUNGER,
                action: EffectAction::ExhaustPlayer { amount: 0.005 },
            },
            PublicationFact::HealthChanged {
                effect_id: REGENERATION,
                amount: 1.0,
                before: 10.0,
                after: 11.0,
            },
        ]
    );
}

#[test]
fn hunger_and_saturation_publish_actions_only_for_players() {
    for (target_kind, expected_actions) in [(TargetKind::NonPlayer, 0), (TargetKind::Player, 2)] {
        let mut state = alive(10.0);
        let mut active = effects(2, 0);
        active
            .add(effect(HUNGER, EffectKind::Hunger, 2, 0))
            .unwrap();
        active
            .add(effect(SATURATION, EffectKind::Saturation, 2, 1))
            .unwrap();
        let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();
        let order = [HUNGER, SATURATION];
        let mut tick_input = ordered_input(&[], &order);
        tick_input.target_kind = target_kind;

        prepare_tick(&state, &active, tick_input, &mut scratch).unwrap();
        let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
        assert_eq!(applied.publications().len(), expected_actions);
        assert!(
            applied
                .publications()
                .iter()
                .all(|fact| matches!(fact, PublicationFact::ExternalEffectAction { .. }))
        );
    }
}

#[test]
fn poison_and_wither_plans_bind_exact_vanilla_damage_sources() {
    for (id, kind, duration, expected_source) in [
        (POISON, EffectKind::Poison, 25, EffectDamageSource::Magic),
        (WITHER, EffectKind::Wither, 40, EffectDamageSource::Wither),
    ] {
        let mut state = alive(10.0);
        let mut active = effects(1, 0);
        active.add(effect(id, kind, duration, 0)).unwrap();
        let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();
        let order = [id];

        let mut prepared =
            prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
        assert_eq!(prepared.actions()[0].damage_source(), Some(expected_source));
        prepared.actions_mut()[0]
            .resolve_damage(DamageContext::default())
            .unwrap();

        let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
        assert!(matches!(
            applied.publications().first(),
            Some(PublicationFact::DamageApplied {
                origin: DamageOrigin::Effect(effect_id),
                source: RuntimeDamageSource::Effect(source),
                ..
            }) if *effect_id == id && *source == expected_source
        ));
    }
}

#[test]
fn queued_damage_callbacks_use_the_pre_expiry_effect_set_in_caller_order() {
    let mut state = alive(10.0);
    let mut active = effects(2, 0);
    let expiring = effect(HUNGER, EffectKind::Hunger, 1, 2);
    let remaining = effect(REGENERATION, EffectKind::Regeneration, 49, 1);
    active.add(expiring).unwrap();
    active.add(remaining).unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 1).unwrap();
    let queued = [generic_damage(1.0)];
    let order = [HUNGER, REGENERATION];
    let mut tick_input = ordered_input(&queued, &order);
    tick_input.target_kind = TargetKind::NonPlayer;

    prepare_tick(&state, &active, tick_input, &mut scratch).unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    let expired = effect(HUNGER, EffectKind::Hunger, 0, 2);
    assert!(matches!(
        applied.publications(),
        [
            PublicationFact::DamageApplied { .. },
            PublicationFact::HurtEvent { .. },
            PublicationFact::EffectHurtCallback {
                effect_id: HUNGER,
                amplifier: 2,
                origin: DamageOrigin::Queued { index: 0 },
                ..
            },
            PublicationFact::EffectHurtCallback {
                effect_id: REGENERATION,
                amplifier: 1,
                origin: DamageOrigin::Queued { index: 0 },
                ..
            },
            PublicationFact::EffectRemoved { effect }
        ] if *effect == expired
    ));
}

#[test]
fn effect_damage_callbacks_exclude_effects_expired_by_earlier_steps() {
    let mut state = alive(10.0);
    let mut active = effects(2, 0);
    let expired = effect(HUNGER, EffectKind::Hunger, 1, 0);
    active.add(expired).unwrap();
    active
        .add(effect(WITHER, EffectKind::Wither, 40, 3))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();
    let order = [HUNGER, WITHER];
    let mut tick_input = ordered_input(&[], &order);
    tick_input.target_kind = TargetKind::NonPlayer;

    let mut prepared = prepare_tick(&state, &active, tick_input, &mut scratch).unwrap();
    prepared.actions_mut()[1]
        .resolve_damage(DamageContext::default())
        .unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert!(matches!(
        applied.publications(),
        [
            PublicationFact::EffectRemoved { .. },
            PublicationFact::DamageApplied { .. },
            PublicationFact::HurtEvent { .. },
            PublicationFact::EffectHurtCallback {
                effect_id: WITHER,
                amplifier: 3,
                origin: DamageOrigin::Effect(WITHER),
                ..
            }
        ]
    ));
}

#[test]
fn effect_damage_callbacks_use_an_earlier_restored_effect_amplifier() {
    let mut state = alive(10.0);
    let mut active = effects(2, 1);
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 601, 0))
        .unwrap();
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 1, 2))
        .unwrap();
    active
        .add(effect(WITHER, EffectKind::Wither, 40, 1))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();
    let order = [REGENERATION, WITHER];

    let mut prepared =
        prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    prepared.actions_mut()[1]
        .resolve_damage(DamageContext::default())
        .unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert!(matches!(
        applied.publications(),
        [
            PublicationFact::EffectRestored { effect, .. },
            PublicationFact::EffectPeriodicSync { .. },
            PublicationFact::DamageApplied { .. },
            PublicationFact::HurtEvent { .. },
            PublicationFact::EffectHurtCallback {
                effect_id: REGENERATION,
                amplifier: 0,
                ..
            },
            PublicationFact::EffectHurtCallback {
                effect_id: WITHER,
                amplifier: 1,
                ..
            }
        ] if effect.amplifier == 0 && effect.duration == 600
    ));
}

#[test]
fn rejected_and_health_gated_damage_do_not_publish_effect_hurt_callbacks() {
    let mut living = LivingState::new(10.0, 0.0).unwrap();
    living.invulnerable_time = 20;
    living.last_hurt = 5.0;
    let mut state = RuntimeState::try_new(living, None).unwrap();
    let mut active = effects(1, 0);
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 49, 0))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 1).unwrap();
    let queued = [generic_damage(1.0)];
    let order = [REGENERATION];
    prepare_tick(
        &state,
        &active,
        ordered_input(&queued, &order),
        &mut scratch,
    )
    .unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert!(matches!(
        applied.publications(),
        [PublicationFact::DamageRejected { .. }]
    ));

    let mut state = alive(1.0);
    let mut active = effects(1, 0);
    active
        .add(effect(POISON, EffectKind::Poison, 25, 0))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();
    let order = [POISON];
    let mut prepared =
        prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    prepared.actions_mut()[0]
        .resolve_damage(DamageContext::default())
        .unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert!(applied.publications().is_empty());
}

#[test]
fn effect_damage_death_event_precedes_effect_expiry_publication() {
    let mut state = alive(1.0);
    let mut active = effects(1, 0);
    let wither = effect(WITHER, EffectKind::Wither, 1, 6);
    active.add(wither).unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();

    let order = [WITHER];
    let mut prepared =
        prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
    prepared.actions_mut()[0]
        .resolve_damage(DamageContext::default())
        .unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    let expired_wither = effect(WITHER, EffectKind::Wither, 0, 6);
    assert!(matches!(
        applied.publications(),
        [
            PublicationFact::DamageApplied {
                origin: DamageOrigin::Effect(WITHER),
                ..
            },
            PublicationFact::HurtEvent {
                origin: DamageOrigin::Effect(WITHER),
                ..
            },
            PublicationFact::DeathStarted {
                origin: DamageOrigin::Effect(WITHER),
                ..
            },
            PublicationFact::EffectHurtCallback {
                effect_id: WITHER,
                amplifier: 6,
                origin: DamageOrigin::Effect(WITHER),
                source: RuntimeDamageSource::Effect(EffectDamageSource::Wither),
                damage: 1.0,
            },
            PublicationFact::EffectRemoved { effect }
        ] if *effect == expired_wither
    ));
}

#[test]
fn runtime_state_and_removal_reason_invariants_are_explicit() {
    let removed = LivingState {
        health: 0.0,
        absorption: 0.0,
        invulnerable_time: 0,
        hurt_time: 0,
        last_hurt: 0.0,
        lifecycle: LivingLifecycle::Removed,
        death_time: 20,
    };
    assert_eq!(
        RuntimeState::try_new(removed, None).unwrap_err(),
        RuntimeStateError::RemovedWithoutReason
    );
    let directly_killed = RuntimeState::try_new(
        LivingState::new(10.0, 0.0).unwrap(),
        Some(RemovalReason::Killed),
    )
    .unwrap();
    assert_eq!(
        directly_killed.removal_reason(),
        Some(RemovalReason::Killed)
    );
    let discarded = RuntimeState::try_new(
        LivingState::new(10.0, 0.0).unwrap(),
        Some(RemovalReason::Discarded),
    )
    .unwrap();
    assert_eq!(discarded.removal_reason(), Some(RemovalReason::Discarded));
    assert_eq!(
        RuntimeState::try_new(
            LivingState {
                health: f32::NAN,
                ..LivingState::new(10.0, 0.0).unwrap()
            },
            None,
        )
        .unwrap_err(),
        RuntimeStateError::InvalidLiving(StateError::NonFiniteHealth)
    );

    assert!(RemovalReason::Killed.should_destroy());
    assert!(!RemovalReason::Killed.should_save());
    assert!(RemovalReason::Discarded.should_destroy());
    assert!(RemovalReason::Killed.triggers_effect_removal_callbacks());
    assert!(RemovalReason::Discarded.triggers_effect_removal_callbacks());
    assert!(RemovalReason::UnloadedToChunk.should_save());
    assert!(!RemovalReason::UnloadedToChunk.triggers_effect_removal_callbacks());
    assert!(!RemovalReason::UnloadedWithPlayer.should_save());
    assert!(!RemovalReason::UnloadedWithPlayer.triggers_effect_removal_callbacks());
    assert!(!RemovalReason::ChangedDimension.should_destroy());
    assert!(!RemovalReason::ChangedDimension.triggers_effect_removal_callbacks());
}

#[test]
fn removed_rows_and_runtime_scratch_hard_caps_fail_before_planning() {
    let removed_living = LivingState {
        health: 0.0,
        absorption: 0.0,
        invulnerable_time: 0,
        hurt_time: 0,
        last_hurt: 0.0,
        lifecycle: LivingLifecycle::Removed,
        death_time: 20,
    };
    let removed = RuntimeState::try_new(removed_living, Some(RemovalReason::Discarded)).unwrap();
    let active = effects(0, 0);
    let mut scratch = RuntimeScratch::try_new(0, 0).unwrap();
    assert_eq!(
        prepare_tick(&removed, &active, input(&[]), &mut scratch).unwrap_err(),
        PrepareError::Removed(RemovalReason::Discarded)
    );
    assert_eq!(
        RuntimeScratch::try_new(0, MAX_DAMAGE_INPUTS_PER_TICK + 1).unwrap_err(),
        RuntimeScratchError::TooManyDamageInputs
    );
    assert_eq!(
        RuntimeScratch::try_new(MAX_ACTIVE_EFFECTS + 1, 0).unwrap_err(),
        RuntimeScratchError::EffectScratch(TickScratchError::CapacityExceedsHardCap)
    );
}

#[test]
fn stale_effects_abort_death_completion_before_runtime_mutation() {
    let living = LivingState {
        health: 0.0,
        absorption: 0.0,
        invulnerable_time: 0,
        hurt_time: 0,
        last_hurt: 1.0,
        lifecycle: LivingLifecycle::Dying,
        death_time: 19,
    };
    let mut state = RuntimeState::try_new(living, None).unwrap();
    let before_state = state;
    let mut active = effects(1, 0);
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, 5, 0))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 0).unwrap();
    let order = [REGENERATION];
    prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();

    let updated = effect(REGENERATION, EffectKind::Regeneration, 10, 0);
    active.add(updated).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::EffectCommit(TickCommitError::StalePlan)
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(REGENERATION), Some(updated));
}

#[test]
fn discard_is_revision_fenced_and_publishes_only_discard_callbacks() {
    let mut state = alive(10.0);
    let mut active = effects(2, 0);
    let regeneration = effect(REGENERATION, EffectKind::Regeneration, 50, 1);
    let caller_owned = effect(CALLER_OWNED, EffectKind::CallerOwned, 5, 2);
    active.add(regeneration).unwrap();
    active.add(caller_owned).unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();
    let order = [CALLER_OWNED, REGENERATION];
    let mut discard = ordered_input(&[], &order);
    discard.mode = TickMode::Discard;

    prepare_tick(&state, &active, discard, &mut scratch).unwrap();
    let expected_revision = state.revision();
    state.replace_living(state.living()).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::StaleState {
            expected: expected_revision,
            actual: state.revision(),
        }
    );
    assert_eq!(active.get(REGENERATION), Some(regeneration));
    assert_eq!(active.get(CALLER_OWNED), Some(caller_owned));

    prepare_tick(&state, &active, discard, &mut scratch).unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert_eq!(
        applied.publications(),
        [
            PublicationFact::EffectRemovalCallback {
                effect: caller_owned,
                reason: RemovalReason::Discarded,
            },
            PublicationFact::EffectRemovalCallback {
                effect: regeneration,
                reason: RemovalReason::Discarded,
            },
            PublicationFact::EntityRemoved {
                reason: RemovalReason::Discarded,
            },
        ]
    );
    assert_eq!(state.living().lifecycle, LivingLifecycle::Alive);
    assert_eq!(state.living().health, 10.0);
    assert_eq!(state.removal_reason(), Some(RemovalReason::Discarded));
    assert!(active.is_empty());
}

#[test]
fn direct_kill_is_revision_fenced_and_publishes_callbacks_before_removal() {
    let mut state = alive(10.0);
    let mut active = effects(2, 0);
    let regeneration = effect(REGENERATION, EffectKind::Regeneration, 50, 1);
    let caller_owned = effect(CALLER_OWNED, EffectKind::CallerOwned, 5, 2);
    active.add(regeneration).unwrap();
    active.add(caller_owned).unwrap();
    let mut scratch = RuntimeScratch::try_new(2, 0).unwrap();
    let scratch_capacities = scratch.capacities();
    let effect_capacities = active.capacities();
    let order = [CALLER_OWNED, REGENERATION];
    let mut kill = ordered_input(&[], &order);
    kill.mode = TickMode::Kill;

    prepare_tick(&state, &active, kill, &mut scratch).unwrap();
    let expected_revision = state.revision();
    state.replace_living(state.living()).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::StaleState {
            expected: expected_revision,
            actual: state.revision(),
        }
    );
    assert_eq!(active.get(REGENERATION), Some(regeneration));
    assert_eq!(active.get(CALLER_OWNED), Some(caller_owned));

    prepare_tick(&state, &active, kill, &mut scratch).unwrap();
    let applied = apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert_eq!(
        applied.publications(),
        [
            PublicationFact::EffectRemovalCallback {
                effect: caller_owned,
                reason: RemovalReason::Killed,
            },
            PublicationFact::EffectRemovalCallback {
                effect: regeneration,
                reason: RemovalReason::Killed,
            },
            PublicationFact::EntityRemoved {
                reason: RemovalReason::Killed,
            },
            PublicationFact::EntityDeathGameEvent,
        ]
    );
    assert_eq!(state.living().lifecycle, LivingLifecycle::Alive);
    assert_eq!(state.living().health, 10.0);
    assert_eq!(state.removal_reason(), Some(RemovalReason::Killed));
    assert!(active.is_empty());
    assert_eq!(scratch.capacities(), scratch_capacities);
    assert_eq!(active.capacities(), effect_capacities);
}

#[test]
fn direct_kill_rejects_stale_effects_and_queued_damage_atomically() {
    let mut state = alive(10.0);
    let before_state = state;
    let mut active = effects(1, 0);
    let original = effect(REGENERATION, EffectKind::Regeneration, 50, 0);
    active.add(original).unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 1).unwrap();
    let order = [REGENERATION];
    let mut kill = ordered_input(&[], &order);
    kill.mode = TickMode::Kill;

    prepare_tick(&state, &active, kill, &mut scratch).unwrap();
    let updated = effect(REGENERATION, EffectKind::Regeneration, 100, 0);
    active.add(updated).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::EffectCommit(TickCommitError::StalePlan)
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(REGENERATION), Some(updated));

    let queued = [generic_damage(1.0)];
    let mut kill_with_damage = ordered_input(&queued, &order);
    kill_with_damage.mode = TickMode::Kill;
    assert_eq!(
        prepare_tick(&state, &active, kill_with_damage, &mut scratch).unwrap_err(),
        PrepareError::KillWithDamage
    );
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::NoPreparedTick
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(REGENERATION), Some(updated));

    let mut bounded = RuntimeScratch::try_new(0, 0).unwrap();
    assert_eq!(
        prepare_tick(&state, &active, kill, &mut bounded).unwrap_err(),
        PrepareError::EffectPlan(TickPlanError::ScratchTooSmall {
            needed: 1,
            capacity: 0,
        })
    );
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut bounded).unwrap_err(),
        ApplyError::NoPreparedTick
    );
    assert_eq!(state, before_state);
    assert_eq!(active.get(REGENERATION), Some(updated));
}

#[test]
fn discard_rejects_queued_damage_without_preparing_partial_work() {
    let mut state = alive(10.0);
    let before_state = state;
    let mut active = effects(0, 0);
    let mut scratch = RuntimeScratch::try_new(0, 1).unwrap();
    let queued = [generic_damage(1.0)];
    let mut discard = input(&queued);
    discard.mode = TickMode::Discard;

    assert_eq!(
        prepare_tick(&state, &active, discard, &mut scratch).unwrap_err(),
        PrepareError::DiscardWithDamage
    );
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::NoPreparedTick
    );
    assert_eq!(state, before_state);
}

#[test]
fn successful_apply_consumes_the_prepared_transaction() {
    let mut state = alive(10.0);
    let mut active = effects(0, 0);
    let mut scratch = RuntimeScratch::try_new(0, 0).unwrap();
    prepare_tick(&state, &active, input(&[]), &mut scratch).unwrap();
    apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    assert_eq!(
        apply_tick(&mut state, &mut active, &mut scratch).unwrap_err(),
        ApplyError::NoPreparedTick
    );
}

#[test]
fn warmed_tick_scratch_and_effect_store_do_not_grow() {
    let mut state = alive(10.0);
    let mut active = effects(1, 0);
    active
        .add(effect(REGENERATION, EffectKind::Regeneration, -1, 0))
        .unwrap();
    let mut scratch = RuntimeScratch::try_new(1, 1).unwrap();
    let scratch_capacities = scratch.capacities();
    let effect_capacities = active.capacities();

    for _ in 0..16 {
        let order = [REGENERATION];
        prepare_tick(&state, &active, ordered_input(&[], &order), &mut scratch).unwrap();
        apply_tick(&mut state, &mut active, &mut scratch).unwrap();
    }

    assert_eq!(scratch.capacities(), scratch_capacities);
    assert_eq!(active.capacities(), effect_capacities);
}
