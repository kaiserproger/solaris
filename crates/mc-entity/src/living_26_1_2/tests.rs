use super::*;

fn alive(health: f32, absorption: f32) -> LivingState {
    LivingState::new(health, absorption).expect("valid living state")
}

fn event(kind: DamageSourceKind, amount: f32) -> DamageEvent {
    DamageEvent {
        source: DamageSource::vanilla(kind),
        amount,
    }
}

fn apply(state: &mut LivingState, event: DamageEvent, context: DamageContext) -> DamageOutcome {
    let mut output = DamageOutcome::Rejected(DamageRejection::Dead);
    apply_damage(state, event, context, &mut output);
    output
}

fn applied(outcome: DamageOutcome) -> DamageApplied {
    match outcome {
        DamageOutcome::Applied(applied) => applied,
        DamageOutcome::Rejected(rejection) => panic!("damage rejected: {rejection:?}"),
    }
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1.0e-6,
        "expected {expected:?}, got {actual:?}"
    );
}

#[test]
fn canonical_sources_match_the_local_26_1_2_damage_type_tags() {
    let fall = DamageSource::vanilla(DamageSourceKind::Fall).flags();
    assert!(fall.contains(DamageFlags::BYPASSES_ARMOR));
    assert!(fall.contains(DamageFlags::IS_FALL));
    assert!(fall.contains(DamageFlags::NO_KNOCKBACK));

    let fire = DamageSource::vanilla(DamageSourceKind::Fire).flags();
    assert!(fire.contains(DamageFlags::IS_FIRE));
    assert!(fire.contains(DamageFlags::NO_KNOCKBACK));
    assert!(!fire.contains(DamageFlags::BYPASSES_ARMOR));

    let lava = DamageSource::vanilla(DamageSourceKind::Lava).flags();
    assert!(lava.contains(DamageFlags::IS_FIRE));
    assert!(lava.contains(DamageFlags::NO_KNOCKBACK));
    assert!(!lava.contains(DamageFlags::BYPASSES_ARMOR));

    let drowning = DamageSource::vanilla(DamageSourceKind::Drowning).flags();
    assert!(drowning.contains(DamageFlags::BYPASSES_ARMOR));
    assert!(drowning.contains(DamageFlags::IS_DROWNING));
    assert!(drowning.contains(DamageFlags::NO_IMPACT));
    assert!(drowning.contains(DamageFlags::NO_KNOCKBACK));

    let suffocation = DamageSource::vanilla(DamageSourceKind::Suffocation).flags();
    assert!(suffocation.contains(DamageFlags::BYPASSES_ARMOR));
    assert!(suffocation.contains(DamageFlags::NO_KNOCKBACK));
    assert!(!suffocation.contains(DamageFlags::BYPASSES_EFFECTS));

    let starvation = DamageSource::vanilla(DamageSourceKind::Starvation).flags();
    assert!(starvation.contains(DamageFlags::BYPASSES_ARMOR));
    assert!(starvation.contains(DamageFlags::BYPASSES_EFFECTS));
    assert!(starvation.contains(DamageFlags::NO_KNOCKBACK));

    let void = DamageSource::vanilla(DamageSourceKind::Void).flags();
    assert!(void.contains(DamageFlags::BYPASSES_ARMOR));
    assert!(void.contains(DamageFlags::BYPASSES_INVULNERABILITY));
    assert!(void.contains(DamageFlags::BYPASSES_RESISTANCE));
    assert!(void.contains(DamageFlags::NO_KNOCKBACK));

    let generic = DamageSource::vanilla(DamageSourceKind::Generic).flags();
    assert!(generic.contains(DamageFlags::BYPASSES_ARMOR));
    assert!(generic.contains(DamageFlags::NO_KNOCKBACK));
    assert!(!generic.contains(DamageFlags::BYPASSES_INVULNERABILITY));

    let indirect_magic = DamageSource::vanilla(DamageSourceKind::IndirectMagic).flags();
    assert!(indirect_magic.contains(DamageFlags::BYPASSES_ARMOR));
    assert!(!indirect_magic.contains(DamageFlags::BYPASSES_EFFECTS));
    assert!(!indirect_magic.contains(DamageFlags::BYPASSES_RESISTANCE));
    assert!(!indirect_magic.contains(DamageFlags::NO_KNOCKBACK));

    let melee = DamageSource::vanilla(DamageSourceKind::Melee).flags();
    assert_eq!(melee, DamageFlags::NONE);

    let projectile = DamageSource::vanilla(DamageSourceKind::Projectile).flags();
    assert!(projectile.contains(DamageFlags::IS_PROJECTILE));
    assert!(!projectile.contains(DamageFlags::NO_KNOCKBACK));
}

#[test]
fn common_damage_sources_share_reduction_and_immunity_boundaries() {
    let reductions = ReductionContext {
        armor: 20,
        armor_toughness: 8.0,
        armor_effectiveness: ArmorEffectiveness::Unmodified,
        resistance: None,
        enchantment: EnchantmentProtection::None,
    };
    for (kind, armor_bypassed, no_knockback) in [
        (DamageSourceKind::Melee, false, false),
        (DamageSourceKind::Projectile, false, false),
        (DamageSourceKind::IndirectMagic, true, false),
        (DamageSourceKind::Fire, false, true),
        (DamageSourceKind::Lava, false, true),
        (DamageSourceKind::Fall, true, true),
        (DamageSourceKind::Drowning, true, true),
        (DamageSourceKind::Suffocation, true, true),
        (DamageSourceKind::Starvation, true, true),
    ] {
        let mut state = alive(20.0, 0.0);
        let result = applied(apply(
            &mut state,
            event(kind, 10.0),
            DamageContext {
                reductions,
                ..DamageContext::default()
            },
        ));
        assert_eq!(result.after_armor == 10.0, armor_bypassed, "{kind:?}");
        assert_eq!(
            result.knockback == KnockbackOutcome::None,
            no_knockback,
            "{kind:?}"
        );
    }

    for kind in [DamageSourceKind::Fire, DamageSourceKind::Lava] {
        let mut immune = alive(20.0, 0.0);
        assert_eq!(
            apply(
                &mut immune,
                event(kind, 2.0),
                DamageContext {
                    immunity: TargetImmunityContext {
                        fire_immune: true,
                        ..TargetImmunityContext::default()
                    },
                    ..DamageContext::default()
                },
            ),
            DamageOutcome::Rejected(DamageRejection::FireImmune),
            "{kind:?}"
        );
        let mut resistant = alive(20.0, 0.0);
        assert_eq!(
            apply(
                &mut resistant,
                event(kind, 2.0),
                DamageContext {
                    fire_resistance: true,
                    ..DamageContext::default()
                },
            ),
            DamageOutcome::Rejected(DamageRejection::FireResistance),
            "{kind:?}"
        );
    }

    let mut fall_immune = alive(20.0, 0.0);
    assert_eq!(
        apply(
            &mut fall_immune,
            event(DamageSourceKind::Fall, 2.0),
            DamageContext {
                immunity: TargetImmunityContext {
                    fall_damage_immune: true,
                    ..TargetImmunityContext::default()
                },
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Rejected(DamageRejection::FallDamageImmune)
    );
}

#[test]
fn fresh_melee_hit_updates_scalar_hurt_state_and_returns_deltas_only() {
    let mut state = alive(20.0, 0.0);
    let context = DamageContext {
        knockback: Some(KnockbackInput {
            direction_x: 1.0,
            direction_z: 0.0,
            velocity_x: 0.2,
            velocity_y: 0.1,
            velocity_z: -0.4,
            on_ground: true,
            resistance: 0.0,
        }),
        ..DamageContext::default()
    };

    let result = applied(apply(
        &mut state,
        event(DamageSourceKind::Melee, 5.0),
        context,
    ));

    assert_eq!(state.health, 15.0);
    assert_eq!(state.invulnerable_time, 20);
    assert_eq!(state.hurt_time, 10);
    assert_eq!(state.last_hurt, 5.0);
    assert_eq!(state.lifecycle, LivingLifecycle::Alive);
    assert_eq!(result.raw_amount, 5.0);
    assert_eq!(result.cooldown_damage, 5.0);
    assert_eq!(result.health_damage, 5.0);
    assert_eq!(result.health_lost, 5.0);
    assert!(result.fresh_hurt);
    assert!(result.mark_hurt);
    assert_eq!(result.lifecycle, LifecycleTransition::None);
    assert_eq!(
        result.knockback,
        KnockbackOutcome::Velocity(KnockbackVelocity {
            x: -0.300_000_005_960_464_5,
            y: 0.4,
            z: -0.2,
        })
    );
}

#[test]
fn cooldown_rejects_equal_damage_and_applies_only_a_stronger_difference() {
    let mut state = alive(20.0, 0.0);
    let first = applied(apply(
        &mut state,
        event(DamageSourceKind::Melee, 5.0),
        DamageContext::default(),
    ));
    assert!(first.fresh_hurt);
    let after_first = state;

    assert_eq!(
        apply(
            &mut state,
            event(DamageSourceKind::Melee, 5.0),
            DamageContext::default(),
        ),
        DamageOutcome::Rejected(DamageRejection::HurtCooldown)
    );
    assert_eq!(state, after_first);

    let stronger = applied(apply(
        &mut state,
        event(DamageSourceKind::Melee, 7.0),
        DamageContext::default(),
    ));
    assert_eq!(state.health, 13.0);
    assert_eq!(state.last_hurt, 7.0);
    assert_eq!(state.invulnerable_time, 20);
    assert_eq!(state.hurt_time, 10);
    assert_eq!(stronger.cooldown_damage, 2.0);
    assert!(!stronger.fresh_hurt);
    assert!(!stronger.mark_hurt);
    assert_eq!(stronger.knockback, KnockbackOutcome::None);
}

#[test]
fn cooldown_boundaries_are_strictly_above_ten_and_bypass_is_fresh() {
    let mut protected = alive(20.0, 0.0);
    protected.invulnerable_time = 11;
    protected.last_hurt = 4.0;
    assert_eq!(
        apply(
            &mut protected,
            event(DamageSourceKind::Melee, 4.0),
            DamageContext::default(),
        ),
        DamageOutcome::Rejected(DamageRejection::HurtCooldown)
    );

    let mut boundary = protected;
    boundary.invulnerable_time = 10;
    let result = applied(apply(
        &mut boundary,
        event(DamageSourceKind::Melee, 4.0),
        DamageContext::default(),
    ));
    assert!(result.fresh_hurt);
    assert_eq!(boundary.health, 16.0);

    let bypass_source =
        DamageSource::with_flags(DamageSourceKind::Melee, DamageFlags::BYPASSES_COOLDOWN);
    let mut bypassed = protected;
    let result = applied(apply(
        &mut bypassed,
        DamageEvent {
            source: bypass_source,
            amount: 2.0,
        },
        DamageContext::default(),
    ));
    assert!(result.fresh_hurt);
    assert_eq!(bypassed.health, 18.0);
    assert_eq!(bypassed.last_hurt, 2.0);
}

#[test]
fn cooldown_accepts_the_next_representable_float_above_last_hurt() {
    let last = 5.0_f32;
    let next = f32::from_bits(last.to_bits() + 1);
    let mut state = alive(20.0, 0.0);
    state.invulnerable_time = 11;
    state.last_hurt = last;

    let result = applied(apply(
        &mut state,
        event(DamageSourceKind::Generic, next),
        DamageContext::default(),
    ));

    assert_eq!(result.cooldown_damage.to_bits(), (next - last).to_bits());
    assert_eq!(state.last_hurt.to_bits(), next.to_bits());
    assert!(!result.fresh_hurt);
}

#[test]
fn target_invulnerability_and_fire_immunity_have_exact_bypass_behavior() {
    let invulnerable = TargetImmunityContext {
        ordinary_invulnerable: true,
        ..TargetImmunityContext::default()
    };
    let mut state = alive(20.0, 0.0);
    assert_eq!(
        apply(
            &mut state,
            event(DamageSourceKind::Melee, 3.0),
            DamageContext {
                immunity: invulnerable,
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Rejected(DamageRejection::Invulnerable)
    );
    assert_eq!(state, alive(20.0, 0.0));

    let void = applied(apply(
        &mut state,
        event(DamageSourceKind::Void, 3.0),
        DamageContext {
            immunity: invulnerable,
            ..DamageContext::default()
        },
    ));
    assert_eq!(void.health_lost, 3.0);

    for context in [
        DamageContext {
            immunity: TargetImmunityContext {
                fire_immune: true,
                ..TargetImmunityContext::default()
            },
            ..DamageContext::default()
        },
        DamageContext {
            fire_resistance: true,
            ..DamageContext::default()
        },
    ] {
        let mut state = alive(20.0, 0.0);
        assert!(matches!(
            apply(&mut state, event(DamageSourceKind::Fire, 3.0), context,),
            DamageOutcome::Rejected(DamageRejection::FireImmune | DamageRejection::FireResistance)
        ));
        assert_eq!(state, alive(20.0, 0.0));
    }
}

#[test]
fn creative_player_bypasses_only_ordinary_invulnerability() {
    let ordinary = TargetImmunityContext {
        ordinary_invulnerable: true,
        source_creative_player: CreativePlayerStatus::Creative,
        ..TargetImmunityContext::default()
    };
    let mut state = alive(20.0, 0.0);
    assert!(matches!(
        apply(
            &mut state,
            event(DamageSourceKind::Melee, 2.0),
            DamageContext {
                immunity: ordinary,
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Applied(_)
    ));

    for (kind, immunity, rejection) in [
        (
            DamageSourceKind::Fire,
            TargetImmunityContext {
                fire_immune: true,
                source_creative_player: CreativePlayerStatus::Creative,
                ..TargetImmunityContext::default()
            },
            DamageRejection::FireImmune,
        ),
        (
            DamageSourceKind::Fall,
            TargetImmunityContext {
                fall_damage_immune: true,
                source_creative_player: CreativePlayerStatus::Creative,
                ..TargetImmunityContext::default()
            },
            DamageRejection::FallDamageImmune,
        ),
        (
            DamageSourceKind::Melee,
            TargetImmunityContext {
                source_creative_player: CreativePlayerStatus::Creative,
                enchantment: EnchantmentImmunity::Immune,
                ..TargetImmunityContext::default()
            },
            DamageRejection::EnchantmentImmune,
        ),
    ] {
        let mut state = alive(20.0, 0.0);
        assert_eq!(
            apply(
                &mut state,
                event(kind, 2.0),
                DamageContext {
                    immunity,
                    ..DamageContext::default()
                },
            ),
            DamageOutcome::Rejected(rejection)
        );
    }
}

#[test]
fn fall_immunity_requires_is_fall_and_ignores_invulnerability_bypass() {
    let immunity = TargetImmunityContext {
        fall_damage_immune: true,
        ..TargetImmunityContext::default()
    };
    let mut generic = alive(20.0, 0.0);
    assert!(matches!(
        apply(
            &mut generic,
            event(DamageSourceKind::Generic, 2.0),
            DamageContext {
                immunity,
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Applied(_)
    ));

    let bypassed_fall = DamageSource::with_flags(
        DamageSourceKind::Fall,
        DamageFlags::IS_FALL
            .union(DamageFlags::BYPASSES_INVULNERABILITY)
            .union(DamageFlags::BYPASSES_ARMOR)
            .union(DamageFlags::NO_KNOCKBACK),
    );
    let mut fall = alive(20.0, 0.0);
    assert_eq!(
        apply(
            &mut fall,
            DamageEvent {
                source: bypassed_fall,
                amount: 2.0,
            },
            DamageContext {
                immunity,
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Rejected(DamageRejection::FallDamageImmune)
    );
}

#[test]
fn enchantment_immunity_is_not_bypassed_by_source_tags() {
    let immunity = TargetImmunityContext {
        ordinary_invulnerable: true,
        source_creative_player: CreativePlayerStatus::Creative,
        enchantment: EnchantmentImmunity::Immune,
        ..TargetImmunityContext::default()
    };
    for kind in [DamageSourceKind::Melee, DamageSourceKind::Void] {
        let mut state = alive(20.0, 0.0);
        assert_eq!(
            apply(
                &mut state,
                event(kind, 2.0),
                DamageContext {
                    immunity,
                    ..DamageContext::default()
                },
            ),
            DamageOutcome::Rejected(DamageRejection::EnchantmentImmune)
        );
    }
}

#[test]
fn unknown_immunity_inputs_fail_only_when_oracle_would_read_them() {
    let unknown_creative = TargetImmunityContext {
        source_creative_player: CreativePlayerStatus::Unsupported,
        ..TargetImmunityContext::default()
    };
    let mut not_invulnerable = alive(20.0, 0.0);
    assert!(matches!(
        apply(
            &mut not_invulnerable,
            event(DamageSourceKind::Generic, 1.0),
            DamageContext {
                immunity: unknown_creative,
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Applied(_)
    ));

    let mut needs_creative = alive(20.0, 0.0);
    assert_eq!(
        apply(
            &mut needs_creative,
            event(DamageSourceKind::Melee, 1.0),
            DamageContext {
                immunity: TargetImmunityContext {
                    ordinary_invulnerable: true,
                    ..unknown_creative
                },
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Rejected(DamageRejection::Unsupported(
            UnsupportedRule::CreativePlayerStatus
        ))
    );

    let mut tag_bypasses_creative_query = alive(20.0, 0.0);
    assert!(matches!(
        apply(
            &mut tag_bypasses_creative_query,
            event(DamageSourceKind::Void, 1.0),
            DamageContext {
                immunity: TargetImmunityContext {
                    ordinary_invulnerable: true,
                    ..unknown_creative
                },
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Applied(_)
    ));

    let unknown_enchantment = TargetImmunityContext {
        enchantment: EnchantmentImmunity::UnsupportedSourceEvaluation,
        ..TargetImmunityContext::default()
    };
    for kind in [DamageSourceKind::Melee, DamageSourceKind::Void] {
        let mut state = alive(20.0, 0.0);
        assert_eq!(
            apply(
                &mut state,
                event(kind, 1.0),
                DamageContext {
                    immunity: unknown_enchantment,
                    ..DamageContext::default()
                },
            ),
            DamageOutcome::Rejected(DamageRejection::Unsupported(
                UnsupportedRule::EnchantmentImmunityEvaluation
            ))
        );
    }
}

#[test]
fn invulnerability_predicate_short_circuits_before_enchantment_and_death() {
    let unknown_enchantment = EnchantmentImmunity::UnsupportedSourceEvaluation;
    for (kind, immunity, rejection) in [
        (
            DamageSourceKind::Melee,
            TargetImmunityContext {
                ordinary_invulnerable: true,
                enchantment: unknown_enchantment,
                ..TargetImmunityContext::default()
            },
            DamageRejection::Invulnerable,
        ),
        (
            DamageSourceKind::Fire,
            TargetImmunityContext {
                fire_immune: true,
                enchantment: unknown_enchantment,
                ..TargetImmunityContext::default()
            },
            DamageRejection::FireImmune,
        ),
        (
            DamageSourceKind::Fall,
            TargetImmunityContext {
                fall_damage_immune: true,
                enchantment: unknown_enchantment,
                ..TargetImmunityContext::default()
            },
            DamageRejection::FallDamageImmune,
        ),
    ] {
        let mut state = alive(20.0, 0.0);
        assert_eq!(
            apply(
                &mut state,
                event(kind, 1.0),
                DamageContext {
                    immunity,
                    ..DamageContext::default()
                },
            ),
            DamageOutcome::Rejected(rejection)
        );
    }

    let mut dying = LivingState {
        health: 0.0,
        lifecycle: LivingLifecycle::Dying,
        ..alive(1.0, 0.0)
    };
    assert_eq!(
        apply(
            &mut dying,
            event(DamageSourceKind::Melee, 1.0),
            DamageContext {
                immunity: TargetImmunityContext {
                    ordinary_invulnerable: true,
                    ..TargetImmunityContext::default()
                },
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Rejected(DamageRejection::Invulnerable)
    );

    let mut removed = LivingState {
        health: 0.0,
        lifecycle: LivingLifecycle::Removed,
        death_time: 20,
        ..alive(1.0, 0.0)
    };
    assert_eq!(
        apply(
            &mut removed,
            event(DamageSourceKind::Melee, 1.0),
            DamageContext {
                immunity: TargetImmunityContext {
                    enchantment: unknown_enchantment,
                    ..TargetImmunityContext::default()
                },
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Rejected(DamageRejection::Removed)
    );
}

#[test]
fn enchantment_immunity_is_evaluated_before_fire_resistance() {
    let mut state = alive(20.0, 0.0);
    assert_eq!(
        apply(
            &mut state,
            event(DamageSourceKind::Fire, 1.0),
            DamageContext {
                immunity: TargetImmunityContext {
                    enchantment: EnchantmentImmunity::UnsupportedSourceEvaluation,
                    ..TargetImmunityContext::default()
                },
                fire_resistance: true,
                ..DamageContext::default()
            },
        ),
        DamageOutcome::Rejected(DamageRejection::Unsupported(
            UnsupportedRule::EnchantmentImmunityEvaluation
        ))
    );
}

#[test]
fn verified_reduction_order_is_armor_resistance_enchantment_then_absorption() {
    let reductions = ReductionContext {
        armor: 20,
        armor_toughness: 8.0,
        armor_effectiveness: ArmorEffectiveness::Unmodified,
        resistance: Some(ResistanceEffect { amplifier: 0 }),
        enchantment: EnchantmentProtection::OracleAggregate(5.0),
    };
    let mut state = alive(20.0, 1.0);

    let result = applied(apply(
        &mut state,
        event(DamageSourceKind::Melee, 10.0),
        DamageContext {
            reductions,
            ..DamageContext::default()
        },
    ));

    assert_eq!(result.after_armor.to_bits(), 3.0_f32.to_bits());
    assert_close(result.after_magic, 1.92);
    assert_eq!(result.absorbed, 1.0);
    assert_close(result.health_damage, 0.92);
    assert_close(state.health, 19.08);
    assert_eq!(state.absorption, 0.0);
}

#[test]
fn armor_clamps_at_twenty_and_twenty_percent_with_attribute_boundaries() {
    for (amount, armor, toughness, expected) in [
        (1.0, 30, 0.0, 0.199_999_99),
        (100.0, 20, 0.0, 84.0),
        (10.0, 30, 20.0, 2.0),
    ] {
        let mut state = alive(200.0, 0.0);
        let result = applied(apply(
            &mut state,
            event(DamageSourceKind::Melee, amount),
            DamageContext {
                reductions: ReductionContext {
                    armor,
                    armor_toughness: toughness,
                    ..ReductionContext::default()
                },
                ..DamageContext::default()
            },
        ));
        assert_close(result.after_armor, expected);
    }
}

#[test]
fn absorption_equal_and_greater_than_damage_preserve_health_exactly() {
    for (absorption, expected_remaining) in [(5.0, 0.0), (8.0, 3.0)] {
        let mut state = alive(20.0, absorption);
        let result = applied(apply(
            &mut state,
            event(DamageSourceKind::Generic, 5.0),
            DamageContext::default(),
        ));
        assert_eq!(result.absorbed, 5.0);
        assert_eq!(result.health_damage.to_bits(), 0.0_f32.to_bits());
        assert_eq!(result.health_lost.to_bits(), 0.0_f32.to_bits());
        assert_eq!(state.health, 20.0);
        assert_eq!(state.absorption, expected_remaining);
    }
}

#[test]
fn bypass_flags_skip_only_the_oracle_stages_they_name() {
    let unsupported = ReductionContext {
        armor: 20,
        armor_toughness: 8.0,
        armor_effectiveness: ArmorEffectiveness::UnsupportedWeaponEnchantment,
        resistance: Some(ResistanceEffect { amplifier: 4 }),
        enchantment: EnchantmentProtection::UnsupportedSourceEvaluation,
    };

    let mut armor_bypassed = alive(20.0, 0.0);
    let outcome = apply(
        &mut armor_bypassed,
        event(DamageSourceKind::Fall, 5.0),
        DamageContext {
            reductions: ReductionContext {
                enchantment: EnchantmentProtection::None,
                resistance: None,
                ..unsupported
            },
            ..DamageContext::default()
        },
    );
    assert_eq!(applied(outcome).after_armor, 5.0);

    let effects_bypassed_source = DamageSource::with_flags(
        DamageSourceKind::Generic,
        DamageFlags::BYPASSES_ARMOR.union(DamageFlags::BYPASSES_EFFECTS),
    );
    let mut effects_bypassed = alive(20.0, 0.0);
    let result = applied(apply(
        &mut effects_bypassed,
        DamageEvent {
            source: effects_bypassed_source,
            amount: 5.0,
        },
        DamageContext {
            reductions: unsupported,
            ..DamageContext::default()
        },
    ));
    assert_eq!(result.after_magic, 5.0);

    let resistance_bypassed_source = DamageSource::with_flags(
        DamageSourceKind::Generic,
        DamageFlags::BYPASSES_ARMOR.union(DamageFlags::BYPASSES_RESISTANCE),
    );
    let mut resistance_bypassed = alive(20.0, 0.0);
    let result = applied(apply(
        &mut resistance_bypassed,
        DamageEvent {
            source: resistance_bypassed_source,
            amount: 5.0,
        },
        DamageContext {
            reductions: ReductionContext {
                armor_effectiveness: ArmorEffectiveness::Unmodified,
                enchantment: EnchantmentProtection::OracleAggregate(5.0),
                ..unsupported
            },
            ..DamageContext::default()
        },
    ));
    assert_eq!(result.after_magic, 4.0);

    let enchantment_bypassed_source = DamageSource::with_flags(
        DamageSourceKind::Generic,
        DamageFlags::BYPASSES_ARMOR.union(DamageFlags::BYPASSES_ENCHANTMENTS),
    );
    let mut enchantment_bypassed = alive(20.0, 0.0);
    let result = applied(apply(
        &mut enchantment_bypassed,
        DamageEvent {
            source: enchantment_bypassed_source,
            amount: 5.0,
        },
        DamageContext {
            reductions: ReductionContext {
                armor_effectiveness: ArmorEffectiveness::Unmodified,
                resistance: None,
                ..unsupported
            },
            ..DamageContext::default()
        },
    ));
    assert_eq!(result.after_magic, 5.0);
}

#[test]
fn unsupported_dynamic_reductions_reject_without_partial_mutation() {
    for reductions in [
        ReductionContext {
            armor: 5,
            armor_effectiveness: ArmorEffectiveness::UnsupportedWeaponEnchantment,
            ..ReductionContext::default()
        },
        ReductionContext {
            enchantment: EnchantmentProtection::UnsupportedSourceEvaluation,
            ..ReductionContext::default()
        },
    ] {
        let mut state = alive(20.0, 2.0);
        let before = state;
        assert!(matches!(
            apply(
                &mut state,
                event(DamageSourceKind::Melee, 4.0),
                DamageContext {
                    reductions,
                    ..DamageContext::default()
                },
            ),
            DamageOutcome::Rejected(DamageRejection::Unsupported(_))
        ));
        assert_eq!(state, before);
    }
}

#[test]
fn resistance_and_enchantment_clamps_match_java_float_boundaries() {
    let mut fully_resisted = alive(20.0, 0.0);
    let result = applied(apply(
        &mut fully_resisted,
        event(DamageSourceKind::Melee, 7.0),
        DamageContext {
            reductions: ReductionContext {
                resistance: Some(ResistanceEffect { amplifier: 4 }),
                ..ReductionContext::default()
            },
            ..DamageContext::default()
        },
    ));
    assert_eq!(result.after_magic.to_bits(), 0.0_f32.to_bits());
    assert_eq!(fully_resisted.health, 20.0);

    for points in [-1.0, 0.0, 20.0, 21.0] {
        let mut state = alive(20.0, 0.0);
        let result = applied(apply(
            &mut state,
            event(DamageSourceKind::Melee, 10.0),
            DamageContext {
                reductions: ReductionContext {
                    enchantment: EnchantmentProtection::OracleAggregate(points),
                    ..ReductionContext::default()
                },
                ..DamageContext::default()
            },
        ));
        let expected: f32 = if points <= 0.0 { 10.0 } else { 1.999_999_9 };
        assert_eq!(result.after_magic.to_bits(), expected.to_bits());
    }
}

#[test]
fn nonfinite_inputs_and_derived_overflow_reject_without_mutation() {
    for amount in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut state = alive(20.0, 0.0);
        let before = state;
        assert!(matches!(
            apply(
                &mut state,
                event(DamageSourceKind::Melee, amount),
                DamageContext::default(),
            ),
            DamageOutcome::Rejected(DamageRejection::NonFinite(_))
        ));
        assert_eq!(state, before);
    }

    let invalid_contexts = [
        (
            DamageContext {
                reductions: ReductionContext {
                    armor_toughness: f32::INFINITY,
                    ..ReductionContext::default()
                },
                ..DamageContext::default()
            },
            DamageInputField::ArmorToughness,
        ),
        (
            DamageContext {
                reductions: ReductionContext {
                    enchantment: EnchantmentProtection::OracleAggregate(f32::NAN),
                    ..ReductionContext::default()
                },
                ..DamageContext::default()
            },
            DamageInputField::EnchantmentProtection,
        ),
        (
            DamageContext {
                knockback: Some(KnockbackInput {
                    direction_x: f64::INFINITY,
                    ..KnockbackInput::default()
                }),
                ..DamageContext::default()
            },
            DamageInputField::KnockbackDirection,
        ),
        (
            DamageContext {
                knockback: Some(KnockbackInput {
                    velocity_y: f64::NAN,
                    ..KnockbackInput::default()
                }),
                ..DamageContext::default()
            },
            DamageInputField::KnockbackVelocity,
        ),
        (
            DamageContext {
                knockback: Some(KnockbackInput {
                    resistance: f64::INFINITY,
                    ..KnockbackInput::default()
                }),
                ..DamageContext::default()
            },
            DamageInputField::KnockbackResistance,
        ),
        (
            DamageContext {
                knockback: Some(KnockbackInput {
                    direction_x: f64::MAX,
                    ..KnockbackInput::default()
                }),
                ..DamageContext::default()
            },
            DamageInputField::DerivedKnockback,
        ),
    ];
    for (context, field) in invalid_contexts {
        let mut state = alive(20.0, 0.0);
        let before = state;
        assert_eq!(
            apply(&mut state, event(DamageSourceKind::Melee, 2.0), context,),
            DamageOutcome::Rejected(DamageRejection::NonFinite(field))
        );
        assert_eq!(state, before);
    }
}

#[test]
fn finite_max_damage_that_overflows_resistance_is_still_lethal() {
    let mut state = alive(20.0, 2.0);
    let result = applied(apply(
        &mut state,
        event(DamageSourceKind::Melee, f32::MAX),
        DamageContext {
            reductions: ReductionContext {
                resistance: Some(ResistanceEffect { amplifier: 0 }),
                ..ReductionContext::default()
            },
            ..DamageContext::default()
        },
    ));

    assert_eq!(result.after_armor, f32::MAX);
    assert!(result.after_magic.is_infinite() && result.after_magic.is_sign_positive());
    assert!(result.health_damage.is_infinite() && result.health_damage.is_sign_positive());
    assert_eq!(result.absorbed, 2.0);
    assert_eq!(result.health_lost, 20.0);
    assert_eq!(result.lifecycle, LifecycleTransition::StartedDying);
    assert_eq!(state.health, 0.0);
    assert_eq!(state.absorption, 0.0);
    assert_eq!(state.last_hurt, f32::MAX);
    assert_eq!(state.invulnerable_time, 20);
    assert_eq!(state.hurt_time, 10);
    assert_eq!(state.lifecycle, LivingLifecycle::Dying);
}

#[test]
fn reducer_distinguishes_positive_infinity_from_nan() {
    let positive_infinity = super::reduction::reduce_damage(
        f32::INFINITY,
        DamageFlags::BYPASSES_ARMOR,
        ReductionContext::default(),
    )
    .expect("derived positive infinity follows Java float arithmetic");
    assert!(positive_infinity.after_magic.is_infinite());
    assert!(positive_infinity.after_magic.is_sign_positive());

    assert!(
        super::reduction::reduce_damage(
            f32::NAN,
            DamageFlags::BYPASSES_ARMOR,
            ReductionContext::default(),
        )
        .is_err()
    );
    assert!(
        super::reduction::reduce_damage(
            f32::NEG_INFINITY,
            DamageFlags::BYPASSES_ARMOR,
            ReductionContext::default(),
        )
        .is_err()
    );
}

#[test]
fn out_of_range_attributes_reject_at_the_confirmed_vanilla_limits() {
    let contexts = [
        (
            DamageContext {
                reductions: ReductionContext {
                    armor: 31,
                    ..ReductionContext::default()
                },
                ..DamageContext::default()
            },
            DamageInputField::Armor,
        ),
        (
            DamageContext {
                reductions: ReductionContext {
                    armor: 30,
                    armor_toughness: 20.000_002,
                    ..ReductionContext::default()
                },
                ..DamageContext::default()
            },
            DamageInputField::ArmorToughness,
        ),
        (
            DamageContext {
                knockback: Some(KnockbackInput {
                    resistance: 1.000_000_000_000_000_2,
                    ..KnockbackInput::default()
                }),
                ..DamageContext::default()
            },
            DamageInputField::KnockbackResistance,
        ),
    ];

    for (context, field) in contexts {
        let mut state = alive(20.0, 0.0);
        let before = state;
        assert_eq!(
            apply(&mut state, event(DamageSourceKind::Melee, 2.0), context,),
            DamageOutcome::Rejected(DamageRejection::OutOfRange(field))
        );
        assert_eq!(state, before);
    }
}

#[test]
fn negative_and_negative_zero_follow_java_pre_cooldown_clamping() {
    let mut negative = alive(20.0, 0.0);
    let result = applied(apply(
        &mut negative,
        event(DamageSourceKind::Generic, -3.0),
        DamageContext::default(),
    ));
    assert_eq!(result.raw_amount.to_bits(), 0.0_f32.to_bits());
    assert_eq!(negative.last_hurt.to_bits(), 0.0_f32.to_bits());
    assert_eq!(negative.health, 20.0);
    assert!(result.fresh_hurt);

    let mut negative_zero = alive(20.0, 0.0);
    let result = applied(apply(
        &mut negative_zero,
        event(DamageSourceKind::Generic, -0.0),
        DamageContext::default(),
    ));
    assert_eq!(result.raw_amount.to_bits(), (-0.0_f32).to_bits());
    assert_eq!(negative_zero.last_hurt.to_bits(), (-0.0_f32).to_bits());
    assert_eq!(result.after_magic.to_bits(), 0.0_f32.to_bits());
}

#[test]
fn lethal_damage_starts_death_once_and_duplicate_damage_is_idempotent() {
    let mut state = alive(4.0, 1.0);
    let result = applied(apply(
        &mut state,
        event(DamageSourceKind::Generic, 8.0),
        DamageContext::default(),
    ));
    assert_eq!(result.absorbed, 1.0);
    assert_eq!(result.health_damage, 7.0);
    assert_eq!(result.health_lost, 4.0);
    assert_eq!(result.lifecycle, LifecycleTransition::StartedDying);
    assert_eq!(state.health, 0.0);
    assert_eq!(state.lifecycle, LivingLifecycle::Dying);
    assert_eq!(state.death_time, 0);

    let dead = state;
    assert_eq!(
        apply(
            &mut state,
            event(DamageSourceKind::Void, f32::MAX),
            DamageContext::default(),
        ),
        DamageOutcome::Rejected(DamageRejection::Dead)
    );
    assert_eq!(state, dead);
}

#[test]
fn death_time_removes_exactly_on_tick_twenty_and_only_once() {
    let mut state = alive(1.0, 0.0);
    applied(apply(
        &mut state,
        event(DamageSourceKind::Generic, 1.0),
        DamageContext::default(),
    ));

    for expected in 1..20 {
        let mut output = TickOutcome::InvalidState(StateError::AliveWithoutHealth);
        tick_living(&mut state, InvulnerabilityClock::Kernel, &mut output);
        assert_eq!(output, TickOutcome::Advanced(TickApplied::Stable));
        assert_eq!(state.death_time, expected);
        assert_eq!(state.lifecycle, LivingLifecycle::Dying);
    }

    let mut output = TickOutcome::InvalidState(StateError::AliveWithoutHealth);
    tick_living(&mut state, InvulnerabilityClock::Kernel, &mut output);
    assert_eq!(output, TickOutcome::Advanced(TickApplied::RemovedNow));
    assert_eq!(state.death_time, 20);
    assert_eq!(state.lifecycle, LivingLifecycle::Removed);

    tick_living(&mut state, InvulnerabilityClock::Kernel, &mut output);
    assert_eq!(output, TickOutcome::Advanced(TickApplied::Stable));
    assert_eq!(state.death_time, 20);
}

#[test]
fn ticking_decrements_hurt_time_and_uses_an_explicit_player_clock_boundary() {
    let mut kernel = alive(20.0, 0.0);
    kernel.hurt_time = 1;
    kernel.invulnerable_time = 1;
    let mut output = TickOutcome::InvalidState(StateError::AliveWithoutHealth);
    tick_living(&mut kernel, InvulnerabilityClock::Kernel, &mut output);
    assert_eq!(kernel.hurt_time, 0);
    assert_eq!(kernel.invulnerable_time, 0);
    assert_eq!(output, TickOutcome::Advanced(TickApplied::Stable));

    let mut external = alive(20.0, 0.0);
    external.hurt_time = 1;
    external.invulnerable_time = 1;
    tick_living(&mut external, InvulnerabilityClock::External, &mut output);
    assert_eq!(external.hurt_time, 0);
    assert_eq!(external.invulnerable_time, 1);
}

#[test]
fn knockback_reports_the_oracle_random_boundary_without_guessing_rng() {
    let threshold = VANILLA_RANDOM_KNOCKBACK_DIRECTION_SQUARED;
    let below = threshold.sqrt();
    assert!(below * below < threshold);
    let at = f64::from_bits(below.to_bits() + 1);
    assert!(at * at >= threshold);

    for (direction_x, expected) in [
        (below, KnockbackOutcome::RandomDirectionRequired),
        (
            at,
            KnockbackOutcome::Velocity(KnockbackVelocity {
                x: -0.400_000_005_960_464_5,
                y: 0.4,
                z: 0.0,
            }),
        ),
    ] {
        let mut state = alive(20.0, 0.0);
        let result = applied(apply(
            &mut state,
            event(DamageSourceKind::Melee, 1.0),
            DamageContext {
                knockback: Some(KnockbackInput {
                    direction_x,
                    on_ground: true,
                    ..KnockbackInput::default()
                }),
                ..DamageContext::default()
            },
        ));
        assert_eq!(result.knockback, expected);
    }

    let mut no_source_position = alive(20.0, 0.0);
    assert_eq!(
        applied(apply(
            &mut no_source_position,
            event(DamageSourceKind::Melee, 1.0),
            DamageContext::default(),
        ))
        .knockback,
        KnockbackOutcome::RandomDirectionRequired
    );

    let mut fall = alive(20.0, 0.0);
    assert_eq!(
        applied(apply(
            &mut fall,
            event(DamageSourceKind::Fall, 1.0),
            DamageContext::default(),
        ))
        .knockback,
        KnockbackOutcome::None
    );

    let mut resistant = alive(20.0, 0.0);
    let result = applied(apply(
        &mut resistant,
        event(DamageSourceKind::Melee, 1.0),
        DamageContext {
            knockback: Some(KnockbackInput {
                direction_x: 1.0,
                resistance: 1.0,
                ..KnockbackInput::default()
            }),
            ..DamageContext::default()
        },
    ));
    assert_eq!(result.knockback, KnockbackOutcome::None);
}

#[test]
fn no_impact_source_applies_damage_without_marking_hurt() {
    let mut state = alive(20.0, 0.0);
    let result = applied(apply(
        &mut state,
        event(DamageSourceKind::Drowning, 2.0),
        DamageContext::default(),
    ));
    assert!(result.fresh_hurt);
    assert!(!result.mark_hurt);
    assert_eq!(result.health_lost, 2.0);
}

#[test]
fn invalid_or_unsupported_inputs_are_typed_and_leave_output_non_stale() {
    let mut state = alive(20.0, 0.0);
    let mut output = DamageOutcome::Applied(DamageApplied::default());
    apply_damage(
        &mut state,
        event(DamageSourceKind::Unsupported, 1.0),
        DamageContext::default(),
        &mut output,
    );
    assert_eq!(
        output,
        DamageOutcome::Rejected(DamageRejection::Unsupported(UnsupportedRule::DamageSource))
    );

    state.health = f32::NAN;
    apply_damage(
        &mut state,
        event(DamageSourceKind::Generic, 1.0),
        DamageContext::default(),
        &mut output,
    );
    assert_eq!(
        output,
        DamageOutcome::Rejected(DamageRejection::InvalidState(StateError::NonFiniteHealth))
    );
}

#[test]
fn every_lifecycle_state_invariant_has_a_typed_rejection() {
    let base = alive(20.0, 0.0);
    let cases = [
        (
            LivingState {
                absorption: f32::NAN,
                ..base
            },
            StateError::NonFiniteAbsorption,
        ),
        (
            LivingState {
                last_hurt: f32::INFINITY,
                ..base
            },
            StateError::NonFiniteLastHurt,
        ),
        (
            LivingState {
                health: -1.0,
                ..base
            },
            StateError::NegativeHealth,
        ),
        (
            LivingState {
                absorption: -1.0,
                ..base
            },
            StateError::NegativeAbsorption,
        ),
        (
            LivingState {
                health: 0.0,
                ..base
            },
            StateError::AliveWithoutHealth,
        ),
        (
            LivingState {
                death_time: 1,
                ..base
            },
            StateError::AliveWithDeathTime,
        ),
        (
            LivingState {
                lifecycle: LivingLifecycle::Dying,
                ..base
            },
            StateError::DyingWithHealth,
        ),
        (
            LivingState {
                health: 0.0,
                lifecycle: LivingLifecycle::Dying,
                death_time: 20,
                ..base
            },
            StateError::DyingPastRemoval,
        ),
        (
            LivingState {
                lifecycle: LivingLifecycle::Removed,
                death_time: 20,
                ..base
            },
            StateError::RemovedWithHealth,
        ),
        (
            LivingState {
                health: 0.0,
                lifecycle: LivingLifecycle::Removed,
                death_time: 19,
                ..base
            },
            StateError::RemovedBeforeDeathTime,
        ),
    ];

    for (mut state, expected) in cases {
        let mut output = DamageOutcome::Applied(DamageApplied::default());
        apply_damage(
            &mut state,
            event(DamageSourceKind::Generic, 1.0),
            DamageContext::default(),
            &mut output,
        );
        assert_eq!(
            output,
            DamageOutcome::Rejected(DamageRejection::InvalidState(expected))
        );
    }

    let mut invalid_tick = LivingState {
        health: 0.0,
        ..base
    };
    let mut tick_output = TickOutcome::Advanced(TickApplied::Stable);
    tick_living(
        &mut invalid_tick,
        InvulnerabilityClock::Kernel,
        &mut tick_output,
    );
    assert_eq!(
        tick_output,
        TickOutcome::InvalidState(StateError::AliveWithoutHealth)
    );
}

#[test]
fn hot_path_types_are_copy_and_caller_owned() {
    fn assert_copy<T: Copy>() {}

    assert_copy::<LivingState>();
    assert_copy::<DamageEvent>();
    assert_copy::<DamageContext>();
    assert_copy::<DamageOutcome>();
    assert_copy::<TickOutcome>();
}
