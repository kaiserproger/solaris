use super::*;

const REGENERATION: EffectId = EffectId::new(10);
const POISON: EffectId = EffectId::new(11);
const WITHER: EffectId = EffectId::new(12);
const HUNGER: EffectId = EffectId::new(13);
const SATURATION: EffectId = EffectId::new(14);
const INSTANT_HEALTH: EffectId = EffectId::new(15);
const INSTANT_DAMAGE: EffectId = EffectId::new(16);
const CALLER_OWNED: EffectId = EffectId::new(99);

fn flags(ambient: bool, visible: bool, show_icon: bool) -> EffectFlags {
    EffectFlags {
        ambient,
        visible,
        show_icon,
    }
}

fn effect(id: EffectId, kind: EffectKind, duration: i32, amplifier: i32) -> EffectInstance {
    EffectInstance::new(id, kind, duration, amplifier, flags(false, true, true))
}

fn store(active: usize, hidden: usize) -> ActiveEffects {
    ActiveEffects::try_new(EffectLimits::new(active, hidden).unwrap()).unwrap()
}

#[test]
fn constructor_clamps_amplifier_like_mob_effect_instance() {
    assert_eq!(
        effect(REGENERATION, EffectKind::Regeneration, 20, -20).amplifier,
        0
    );
    assert_eq!(
        effect(REGENERATION, EffectKind::Regeneration, 20, 400).amplifier,
        255
    );
}

#[test]
fn equal_weaker_and_stronger_duplicate_ids_follow_java_merge_outcomes() {
    let mut effects = store(2, 4);
    let original = effect(REGENERATION, EffectKind::Regeneration, 100, 1);
    assert_eq!(
        effects.add(original).unwrap(),
        AddOutcome::Added {
            current: original,
            started: original,
        }
    );

    assert_eq!(
        effects.add(original).unwrap(),
        AddOutcome::Unchanged {
            current: original,
            started: original,
        }
    );
    assert_eq!(effects.len(), 1, "a duplicate id updates one active row");

    let weaker_longer = effect(REGENERATION, EffectKind::Regeneration, 200, 0);
    assert_eq!(
        effects.add(weaker_longer).unwrap(),
        AddOutcome::HiddenOnly {
            current: original,
            started: weaker_longer,
        }
    );
    assert_eq!(effects.hidden_depth(REGENERATION), 1);
    assert_eq!(effects.hidden_at(REGENERATION, 0), Some(weaker_longer));

    let stronger_shorter = effect(REGENERATION, EffectKind::Regeneration, 20, 2);
    assert_eq!(
        effects.add(stronger_shorter).unwrap(),
        AddOutcome::Updated {
            current: stronger_shorter,
            started: stronger_shorter,
            refresh_attributes: true,
        }
    );
    assert_eq!(effects.hidden_depth(REGENERATION), 2);
    assert_eq!(effects.hidden_at(REGENERATION, 0), Some(original));
    assert_eq!(effects.hidden_at(REGENERATION, 1), Some(weaker_longer));
}

#[test]
fn equal_amplifier_longer_duration_replaces_duration_and_all_flags() {
    let mut effects = store(1, 0);
    let original = EffectInstance::new(
        REGENERATION,
        EffectKind::Regeneration,
        10,
        1,
        flags(false, true, true),
    );
    effects.add(original).unwrap();

    let takeover = EffectInstance::new(
        REGENERATION,
        EffectKind::Regeneration,
        11,
        1,
        flags(true, false, false),
    );
    assert!(matches!(
        effects.add(takeover).unwrap(),
        AddOutcome::Updated {
            current,
            refresh_attributes: true,
            ..
        } if current == takeover
    ));
}

#[test]
fn weaker_merge_matches_ambient_and_visibility_update_order() {
    let mut effects = store(1, 1);
    let original = EffectInstance::new(
        REGENERATION,
        EffectKind::Regeneration,
        100,
        2,
        flags(false, true, true),
    );
    effects.add(original).unwrap();
    let weaker = EffectInstance::new(
        REGENERATION,
        EffectKind::Regeneration,
        50,
        1,
        flags(true, false, true),
    );

    let AddOutcome::Updated { current, .. } = effects.add(weaker).unwrap() else {
        panic!("visibility change must publish an update");
    };
    assert!(
        !current.flags.ambient,
        "ambient was evaluated before visible"
    );
    assert!(!current.flags.visible);
    assert_eq!(current.duration, 100);
    assert_eq!(current.amplifier, 2);
}

#[test]
fn recursive_hidden_duration_update_is_reported_without_publication() {
    let mut effects = store(1, 1);
    let active = effect(REGENERATION, EffectKind::Regeneration, 100, 2);
    let hidden = effect(REGENERATION, EffectKind::Regeneration, 200, 0);
    effects.add(active).unwrap();
    effects.add(hidden).unwrap();

    let extended = effect(REGENERATION, EffectKind::Regeneration, 300, 0);
    assert_eq!(
        effects.add(extended).unwrap(),
        AddOutcome::HiddenOnly {
            current: active,
            started: extended,
        }
    );
    assert_eq!(effects.hidden_at(REGENERATION, 0), Some(extended));
}

#[test]
fn infinite_duration_participates_in_merge_and_hidden_ticks_exactly() {
    let mut effects = store(1, 1);
    let finite = effect(REGENERATION, EffectKind::Regeneration, 100, 0);
    let infinite = effect(REGENERATION, EffectKind::Regeneration, -1, 0);
    effects.add(finite).unwrap();
    assert!(matches!(
        effects.add(infinite).unwrap(),
        AddOutcome::Updated { current, .. } if current == infinite
    ));

    let stronger_finite = effect(REGENERATION, EffectKind::Regeneration, 1, 1);
    effects.add(stronger_finite).unwrap();
    assert_eq!(effects.hidden_at(REGENERATION, 0), Some(infinite));
    let mut scratch = TickScratch::try_new(1).unwrap();
    effects
        .plan_tick_batch(
            50,
            TargetEffectContext::LIVING,
            &[REGENERATION],
            &mut scratch,
        )
        .unwrap();
    effects.commit_tick_batch(&mut scratch).unwrap();
    assert_eq!(effects.get(REGENERATION), Some(infinite));
}

#[test]
fn stronger_shorter_effect_restores_decremented_hidden_effect() {
    let mut effects = store(1, 1);
    let base = effect(REGENERATION, EffectKind::Regeneration, 10, 0);
    effects.add(base).unwrap();
    effects
        .add(effect(REGENERATION, EffectKind::Regeneration, 1, 2))
        .unwrap();
    let mut scratch = TickScratch::try_new(1).unwrap();

    let plans = effects
        .plan_tick_batch(
            200,
            TargetEffectContext::LIVING,
            &[REGENERATION],
            &mut scratch,
        )
        .unwrap();
    assert_eq!(plans[0].application(), EffectApplication::None);
    let outcomes = effects.commit_tick_batch(&mut scratch).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].restored.unwrap().duration, 9);
    assert!(outcomes[0].refresh_attributes);
    assert!(outcomes[0].removed.is_none());
    assert_eq!(effects.get(REGENERATION).unwrap().duration, 9);
}

#[test]
fn infinite_zero_and_other_negative_durations_have_distinct_lifecycles() {
    let mut effects = store(3, 0);
    effects
        .add(effect(REGENERATION, EffectKind::Regeneration, -1, 0))
        .unwrap();
    effects
        .add(effect(POISON, EffectKind::Poison, 0, 0))
        .unwrap();
    effects
        .add(effect(WITHER, EffectKind::Wither, -2, 0))
        .unwrap();
    let mut scratch = TickScratch::try_new(3).unwrap();

    let plans = effects
        .plan_tick_batch(
            50,
            TargetEffectContext::LIVING,
            &[REGENERATION, POISON, WITHER],
            &mut scratch,
        )
        .unwrap();
    assert_eq!(plans[0].id(), REGENERATION);
    assert!(matches!(
        plans[0].application(),
        EffectApplication::Supported(_)
    ));
    assert_eq!(plans[1].application(), EffectApplication::None);
    assert_eq!(plans[2].application(), EffectApplication::None);
    let outcomes = effects.commit_tick_batch(&mut scratch).unwrap();

    assert_eq!(effects.get(REGENERATION).unwrap().duration, -1);
    assert!(effects.get(POISON).is_none());
    assert!(effects.get(WITHER).is_none());
    assert!(outcomes[1].removed.is_some());
    assert!(outcomes[2].removed.is_some());
}

#[test]
fn active_and_hidden_caps_fail_without_partial_mutation() {
    let mut effects = store(1, 0);
    let first = effect(REGENERATION, EffectKind::Regeneration, 100, 0);
    effects.add(first).unwrap();

    assert_eq!(
        effects
            .add(effect(POISON, EffectKind::Poison, 100, 0))
            .unwrap_err(),
        EffectStoreError::ActiveCapacityExceeded { capacity: 1 }
    );
    assert_eq!(effects.len(), 1);
    assert_eq!(effects.get(REGENERATION), Some(first));

    assert_eq!(
        effects
            .add(effect(REGENERATION, EffectKind::Regeneration, 10, 1))
            .unwrap_err(),
        EffectStoreError::HiddenCapacityExceeded { capacity: 0 }
    );
    assert_eq!(effects.get(REGENERATION), Some(first));
    assert_eq!(effects.hidden_depth(REGENERATION), 0);
}

#[test]
fn duplicate_id_with_different_behavior_is_rejected_atomically() {
    let mut effects = store(1, 1);
    let original = effect(REGENERATION, EffectKind::Regeneration, 100, 0);
    effects.add(original).unwrap();

    assert_eq!(
        effects
            .add(effect(REGENERATION, EffectKind::Poison, 200, 1))
            .unwrap_err(),
        EffectStoreError::KindMismatch {
            id: REGENERATION,
            active: EffectKind::Regeneration,
            incoming: EffectKind::Poison,
        }
    );
    assert_eq!(effects.get(REGENERATION), Some(original));
    assert_eq!(effects.hidden_depth(REGENERATION), 0);
}

#[test]
fn removal_is_typed_and_idempotent_and_releases_hidden_capacity() {
    let mut effects = store(1, 1);
    let base = effect(REGENERATION, EffectKind::Regeneration, 100, 0);
    let strong = effect(REGENERATION, EffectKind::Regeneration, 10, 1);
    effects.add(base).unwrap();
    effects.add(strong).unwrap();

    assert_eq!(
        effects.remove(REGENERATION),
        RemoveOutcome::Removed { effect: strong }
    );
    assert_eq!(effects.remove(REGENERATION), RemoveOutcome::NotPresent);
    assert_eq!(effects.hidden_nodes_in_use(), 0);

    effects.add(base).unwrap();
    effects.add(strong).unwrap();
    assert_eq!(effects.hidden_nodes_in_use(), 1);
}

#[test]
fn limits_and_tick_scratch_caps_are_explicit() {
    assert_eq!(
        EffectLimits::new(MAX_ACTIVE_EFFECTS + 1, 0).unwrap_err(),
        EffectLimitError::TooManyActiveEffects
    );
    assert_eq!(
        EffectLimits::new(0, MAX_HIDDEN_EFFECTS + 1).unwrap_err(),
        EffectLimitError::TooManyHiddenEffects
    );
    assert_eq!(
        TickScratch::try_new(MAX_ACTIVE_EFFECTS + 1).unwrap_err(),
        TickScratchError::CapacityExceedsHardCap
    );
}

#[test]
fn caller_supplied_order_controls_plan_and_outcome_order() {
    let mut effects = store(3, 0);
    effects
        .add(effect(WITHER, EffectKind::Wither, 40, 0))
        .unwrap();
    effects
        .add(effect(REGENERATION, EffectKind::Regeneration, 50, 0))
        .unwrap();
    effects
        .add(effect(POISON, EffectKind::Poison, 25, 0))
        .unwrap();
    let mut scratch = TickScratch::try_new(3).unwrap();
    let order = [WITHER, REGENERATION, POISON];

    let ids: Vec<_> = effects
        .plan_tick_batch(0, TargetEffectContext::LIVING, &order, &mut scratch)
        .unwrap()
        .iter()
        .map(PendingEffectTick::id)
        .collect();
    assert_eq!(ids, order);

    let outcome_ids: Vec<_> = effects
        .commit_tick_batch(&mut scratch)
        .unwrap()
        .iter()
        .map(|outcome| outcome.id)
        .collect();
    assert_eq!(outcome_ids, order);
}

#[test]
fn caller_order_preflight_rejects_invalid_sets_without_mutation() {
    let mut effects = store(2, 0);
    let regeneration = effect(REGENERATION, EffectKind::Regeneration, 50, 0);
    let poison = effect(POISON, EffectKind::Poison, 25, 0);
    effects.add(regeneration).unwrap();
    effects.add(poison).unwrap();
    let mut scratch = TickScratch::try_new(2).unwrap();

    assert_eq!(
        effects
            .plan_tick_batch(
                0,
                TargetEffectContext::LIVING,
                &[REGENERATION],
                &mut scratch,
            )
            .unwrap_err(),
        TickPlanError::OrderLengthMismatch {
            active: 2,
            provided: 1,
        }
    );
    assert_eq!(
        effects
            .plan_tick_batch(
                0,
                TargetEffectContext::LIVING,
                &[REGENERATION, REGENERATION],
                &mut scratch,
            )
            .unwrap_err(),
        TickPlanError::DuplicateEffectId(REGENERATION)
    );
    assert_eq!(
        effects
            .plan_tick_batch(
                0,
                TargetEffectContext::LIVING,
                &[REGENERATION, CALLER_OWNED],
                &mut scratch,
            )
            .unwrap_err(),
        TickPlanError::UnknownEffectId(CALLER_OWNED)
    );
    assert_eq!(effects.get(REGENERATION), Some(regeneration));
    assert_eq!(effects.get(POISON), Some(poison));
    assert_eq!(
        effects.commit_tick_batch(&mut scratch).unwrap_err(),
        TickCommitError::NoPlannedBatch
    );
}

#[test]
fn periodic_rules_emit_exact_supported_actions() {
    let mut effects = store(5, 0);
    effects
        .add(effect(REGENERATION, EffectKind::Regeneration, 50, 0))
        .unwrap();
    effects
        .add(effect(POISON, EffectKind::Poison, 25, 0))
        .unwrap();
    effects
        .add(effect(WITHER, EffectKind::Wither, 40, 0))
        .unwrap();
    effects
        .add(effect(HUNGER, EffectKind::Hunger, 5, 255))
        .unwrap();
    effects
        .add(effect(SATURATION, EffectKind::Saturation, 1, 2))
        .unwrap();
    let mut scratch = TickScratch::try_new(5).unwrap();

    let plans = effects
        .plan_tick_batch(
            0,
            TargetEffectContext::LIVING,
            &[REGENERATION, POISON, WITHER, HUNGER, SATURATION],
            &mut scratch,
        )
        .unwrap();
    assert_eq!(
        plans[0].application(),
        EffectApplication::Supported(EffectAction::HealIfBelowMax { amount: 1.0 })
    );
    assert_eq!(
        plans[1].application(),
        EffectApplication::Supported(EffectAction::MagicDamageIfHealthAbove {
            amount: 1.0,
            minimum_health: 1.0,
        })
    );
    assert_eq!(
        plans[2].application(),
        EffectApplication::Supported(EffectAction::Damage {
            amount: 1.0,
            source: EffectDamageSource::Wither,
        })
    );
    assert_eq!(
        plans[3].application(),
        EffectApplication::Supported(EffectAction::ExhaustPlayer {
            amount: 0.005_f32 * 256.0_f32,
        })
    );
    assert_eq!(
        plans[4].application(),
        EffectApplication::Supported(EffectAction::FeedPlayer {
            food: 3,
            saturation_modifier: 1.0,
        })
    );
}

#[test]
fn java_masks_shift_distances_for_periodic_intervals() {
    let mut effects = store(1, 0);
    effects
        .add(effect(POISON, EffectKind::Poison, 25, 32))
        .unwrap();
    let mut scratch = TickScratch::try_new(1).unwrap();
    assert!(matches!(
        effects
            .plan_tick_batch(0, TargetEffectContext::LIVING, &[POISON], &mut scratch)
            .unwrap()[0]
            .application(),
        EffectApplication::Supported(_)
    ));
}

#[test]
fn instant_health_damage_apply_undead_inversion_and_java_int_shifts() {
    let health = effect(INSTANT_HEALTH, EffectKind::InstantHealth, 1, 0);
    let damage = effect(INSTANT_DAMAGE, EffectKind::InstantDamage, 1, 0);

    assert_eq!(
        plan_instant_application(
            health,
            TargetEffectContext::LIVING,
            1.0,
            InstantDelivery::Direct,
        )
        .unwrap(),
        InstantApplication::Supported(EffectAction::Heal { amount: 4.0 })
    );
    assert_eq!(
        plan_instant_application(
            health,
            TargetEffectContext::INVERTED_HEAL_AND_HARM,
            1.0,
            InstantDelivery::Indirect,
        )
        .unwrap(),
        InstantApplication::Supported(EffectAction::Damage {
            amount: 6.0,
            source: EffectDamageSource::IndirectMagic,
        })
    );
    assert_eq!(
        plan_instant_application(
            damage,
            TargetEffectContext::LIVING,
            1.0,
            InstantDelivery::Direct,
        )
        .unwrap(),
        InstantApplication::Supported(EffectAction::Damage {
            amount: 6.0,
            source: EffectDamageSource::Magic,
        })
    );
    assert_eq!(
        plan_instant_application(
            damage,
            TargetEffectContext::INVERTED_HEAL_AND_HARM,
            1.0,
            InstantDelivery::Direct,
        )
        .unwrap(),
        InstantApplication::Supported(EffectAction::Heal { amount: 4.0 })
    );

    let shifted = effect(INSTANT_DAMAGE, EffectKind::InstantDamage, 1, 30);
    assert_eq!(
        plan_instant_application(
            shifted,
            TargetEffectContext::LIVING,
            1.0,
            InstantDelivery::Direct,
        )
        .unwrap(),
        InstantApplication::Supported(EffectAction::Damage {
            amount: -2_147_483_647_i32 as f32,
            source: EffectDamageSource::Magic,
        })
    );
}

#[test]
fn scaled_instant_amount_uses_java_rounding_and_saturating_double_to_int_cast() {
    let health = effect(INSTANT_HEALTH, EffectKind::InstantHealth, 1, 0);
    assert_eq!(
        plan_instant_application(
            health,
            TargetEffectContext::LIVING,
            0.5,
            InstantDelivery::Direct,
        )
        .unwrap(),
        InstantApplication::Supported(EffectAction::Heal { amount: 2.0 })
    );
    assert_eq!(
        plan_instant_application(
            health,
            TargetEffectContext::LIVING,
            f64::MAX,
            InstantDelivery::Direct,
        )
        .unwrap(),
        InstantApplication::Supported(EffectAction::Heal {
            amount: i32::MAX as f32,
        })
    );
}

#[test]
fn heal_or_harm_nonfinite_scales_use_java_double_to_int_casts() {
    let health = effect(INSTANT_HEALTH, EffectKind::InstantHealth, 1, 0);
    let damage = effect(INSTANT_DAMAGE, EffectKind::InstantDamage, 1, 0);
    for (scale, expected) in [
        (f64::NAN, 0.0),
        (f64::INFINITY, i32::MAX as f32),
        (f64::NEG_INFINITY, i32::MIN as f32),
    ] {
        assert_eq!(
            plan_instant_application(
                health,
                TargetEffectContext::LIVING,
                scale,
                InstantDelivery::Direct,
            )
            .unwrap(),
            InstantApplication::Supported(EffectAction::Heal { amount: expected })
        );
        assert_eq!(
            plan_instant_application(
                damage,
                TargetEffectContext::LIVING,
                scale,
                InstantDelivery::Direct,
            )
            .unwrap(),
            InstantApplication::Supported(EffectAction::Damage {
                amount: expected,
                source: EffectDamageSource::Magic,
            })
        );
    }

    let zero_shifted_heal = effect(INSTANT_HEALTH, EffectKind::InstantHealth, 1, 30);
    assert_eq!(
        plan_instant_application(
            zero_shifted_heal,
            TargetEffectContext::LIVING,
            f64::INFINITY,
            InstantDelivery::Direct,
        )
        .unwrap(),
        InstantApplication::Supported(EffectAction::Heal { amount: 0.0 })
    );

    let negative_shifted_harm = effect(INSTANT_DAMAGE, EffectKind::InstantDamage, 1, 30);
    for (scale, expected) in [
        (f64::INFINITY, i32::MIN as f32),
        (f64::NEG_INFINITY, i32::MAX as f32),
    ] {
        assert_eq!(
            plan_instant_application(
                negative_shifted_harm,
                TargetEffectContext::LIVING,
                scale,
                InstantDelivery::Direct,
            )
            .unwrap(),
            InstantApplication::Supported(EffectAction::Damage {
                amount: expected,
                source: EffectDamageSource::Magic,
            })
        );
    }
}

#[test]
fn saturation_instant_application_ignores_all_scale_values() {
    let saturation = effect(SATURATION, EffectKind::Saturation, 1, 2);
    for scale in [
        -100.0,
        0.0,
        f64::MAX,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        assert_eq!(
            plan_instant_application(
                saturation,
                TargetEffectContext::INVERTED_HEAL_AND_HARM,
                scale,
                InstantDelivery::Indirect,
            )
            .unwrap(),
            InstantApplication::Supported(EffectAction::FeedPlayer {
                food: 3,
                saturation_modifier: 1.0,
            })
        );
    }
}

#[test]
fn unsupported_effects_remain_an_explicit_caller_owned_tick_boundary() {
    let mut effects = store(1, 0);
    let custom = effect(CALLER_OWNED, EffectKind::CallerOwned, 2, 4);
    effects.add(custom).unwrap();
    let mut scratch = TickScratch::try_new(1).unwrap();

    let plans = effects
        .plan_tick_batch(
            123,
            TargetEffectContext::LIVING,
            &[CALLER_OWNED],
            &mut scratch,
        )
        .unwrap();
    assert_eq!(
        plans[0].application(),
        EffectApplication::CallerOwned {
            tick_count: 2,
            amplifier: 4,
        }
    );
    assert_eq!(
        effects.commit_tick_batch(&mut scratch).unwrap_err(),
        TickCommitError::UnresolvedCallerOwned(CALLER_OWNED)
    );
    assert_eq!(effects.get(CALLER_OWNED), Some(custom));

    effects
        .plan_tick_batch(
            123,
            TargetEffectContext::LIVING,
            &[CALLER_OWNED],
            &mut scratch,
        )
        .unwrap()[0]
        .resolve_caller_owned(CallerOwnedResult::Remove)
        .unwrap();
    let outcomes = effects.commit_tick_batch(&mut scratch).unwrap();
    assert_eq!(outcomes[0].removed, Some(custom));
    assert!(effects.get(CALLER_OWNED).is_none());
}

#[test]
fn caller_owned_skip_and_continue_both_decrement_duration() {
    for result in [CallerOwnedResult::Skipped, CallerOwnedResult::Continue] {
        let mut effects = store(1, 0);
        effects
            .add(effect(CALLER_OWNED, EffectKind::CallerOwned, 2, 0))
            .unwrap();
        let mut scratch = TickScratch::try_new(1).unwrap();
        effects
            .plan_tick_batch(
                0,
                TargetEffectContext::LIVING,
                &[CALLER_OWNED],
                &mut scratch,
            )
            .unwrap()[0]
            .resolve_caller_owned(result)
            .unwrap();
        effects.commit_tick_batch(&mut scratch).unwrap();
        assert_eq!(effects.get(CALLER_OWNED).unwrap().duration, 1);
    }
}

#[test]
fn unresolved_caller_owned_preflight_leaves_earlier_supported_effect_unchanged() {
    let mut effects = store(2, 0);
    let regeneration = effect(REGENERATION, EffectKind::Regeneration, 50, 0);
    let custom = effect(CALLER_OWNED, EffectKind::CallerOwned, 2, 0);
    effects.add(regeneration).unwrap();
    effects.add(custom).unwrap();
    let mut scratch = TickScratch::try_new(2).unwrap();
    effects
        .plan_tick_batch(
            0,
            TargetEffectContext::LIVING,
            &[REGENERATION, CALLER_OWNED],
            &mut scratch,
        )
        .unwrap();

    assert_eq!(
        effects.commit_tick_batch(&mut scratch).unwrap_err(),
        TickCommitError::UnresolvedCallerOwned(CALLER_OWNED)
    );
    assert_eq!(effects.get(REGENERATION), Some(regeneration));
    assert_eq!(effects.get(CALLER_OWNED), Some(custom));
}

#[test]
fn tick_preflight_rejects_small_scratch_and_stale_or_reused_batches() {
    let mut effects = store(2, 0);
    effects
        .add(effect(REGENERATION, EffectKind::Regeneration, 2, 0))
        .unwrap();
    effects
        .add(effect(POISON, EffectKind::Poison, 2, 0))
        .unwrap();
    let before = effects.get(REGENERATION).unwrap();
    let mut small = TickScratch::try_new(1).unwrap();
    assert_eq!(
        effects
            .plan_tick_batch(
                0,
                TargetEffectContext::LIVING,
                &[REGENERATION, POISON],
                &mut small,
            )
            .unwrap_err(),
        TickPlanError::ScratchTooSmall {
            needed: 2,
            capacity: 1,
        }
    );
    assert_eq!(effects.get(REGENERATION), Some(before));

    let mut scratch = TickScratch::try_new(2).unwrap();
    effects
        .plan_tick_batch(
            0,
            TargetEffectContext::LIVING,
            &[REGENERATION, POISON],
            &mut scratch,
        )
        .unwrap();
    effects.remove(POISON);
    assert_eq!(
        effects.commit_tick_batch(&mut scratch).unwrap_err(),
        TickCommitError::StalePlan
    );
    assert_eq!(effects.get(REGENERATION), Some(before));

    effects
        .plan_tick_batch(
            0,
            TargetEffectContext::LIVING,
            &[REGENERATION],
            &mut scratch,
        )
        .unwrap();
    effects.commit_tick_batch(&mut scratch).unwrap();
    assert_eq!(
        effects.commit_tick_batch(&mut scratch).unwrap_err(),
        TickCommitError::NoPlannedBatch
    );
}

#[test]
fn resolving_a_supported_application_is_rejected() {
    let mut effects = store(1, 0);
    effects
        .add(effect(REGENERATION, EffectKind::Regeneration, 50, 0))
        .unwrap();
    let mut scratch = TickScratch::try_new(1).unwrap();
    let plans = effects
        .plan_tick_batch(
            0,
            TargetEffectContext::LIVING,
            &[REGENERATION],
            &mut scratch,
        )
        .unwrap();
    assert_eq!(
        plans[0]
            .resolve_caller_owned(CallerOwnedResult::Continue)
            .unwrap_err(),
        TickResolutionError::NotCallerOwned(REGENERATION)
    );
}

#[test]
fn hidden_restore_can_publish_refresh_then_periodic_sync_in_order() {
    let mut effects = store(1, 1);
    effects
        .add(effect(REGENERATION, EffectKind::Regeneration, 601, 0))
        .unwrap();
    effects
        .add(effect(REGENERATION, EffectKind::Regeneration, 1, 1))
        .unwrap();
    let mut scratch = TickScratch::try_new(1).unwrap();
    effects
        .plan_tick_batch(
            0,
            TargetEffectContext::LIVING,
            &[REGENERATION],
            &mut scratch,
        )
        .unwrap();
    let outcomes = effects.commit_tick_batch(&mut scratch).unwrap();

    assert_eq!(outcomes[0].restored.unwrap().duration, 600);
    assert!(outcomes[0].refresh_attributes);
    assert_eq!(outcomes[0].periodic_sync.unwrap().duration, 600);
    assert!(outcomes[0].removed.is_none());
}

#[test]
fn instant_boundary_rejects_known_noninstant_and_preserves_custom_context() {
    let regeneration = effect(REGENERATION, EffectKind::Regeneration, 20, 0);
    assert_eq!(
        plan_instant_application(
            regeneration,
            TargetEffectContext::LIVING,
            1.0,
            InstantDelivery::Direct,
        )
        .unwrap_err(),
        InstantPlanError::NotInstant(EffectKind::Regeneration)
    );

    let custom = effect(CALLER_OWNED, EffectKind::CallerOwned, 20, 7);
    assert_eq!(
        plan_instant_application(
            custom,
            TargetEffectContext::INVERTED_HEAL_AND_HARM,
            0.25,
            InstantDelivery::Indirect,
        )
        .unwrap(),
        InstantApplication::CallerOwned {
            amplifier: 7,
            scale: 0.25,
            delivery: InstantDelivery::Indirect,
            target: TargetEffectContext::INVERTED_HEAL_AND_HARM,
        }
    );
}

#[test]
fn warmed_store_and_scratch_do_not_grow_capacities() {
    let mut effects = store(2, 2);
    let mut scratch = TickScratch::try_new(2).unwrap();
    let store_capacities = effects.capacities();
    let scratch_capacities = scratch.capacities();

    for _ in 0..8 {
        effects
            .add(effect(REGENERATION, EffectKind::Regeneration, 2, 0))
            .unwrap();
        effects
            .add(effect(REGENERATION, EffectKind::Regeneration, 1, 1))
            .unwrap();
        effects
            .add(effect(POISON, EffectKind::Poison, 2, 0))
            .unwrap();
        effects
            .plan_tick_batch(
                0,
                TargetEffectContext::LIVING,
                &[REGENERATION, POISON],
                &mut scratch,
            )
            .unwrap();
        effects.commit_tick_batch(&mut scratch).unwrap();
        effects.remove(REGENERATION);
        effects.remove(POISON);
    }

    assert_eq!(effects.capacities(), store_capacities);
    assert_eq!(scratch.capacities(), scratch_capacities);
}
