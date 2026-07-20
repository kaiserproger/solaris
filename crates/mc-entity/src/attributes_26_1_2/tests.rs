use super::*;

const HEALTH: AttributeId = AttributeId::new(1);
const SPEED: AttributeId = AttributeId::new(2);
const INPUT_FAILURE: AttributeId = AttributeId::new(20);
const INPUT_PREFIX: AttributeId = AttributeId::new(21);
const UNKNOWN: AttributeId = AttributeId::new(99);

fn ranged(id: AttributeId, default_value: f64, syncable: bool) -> AttributeDefinition {
    AttributeDefinition::ranged(id, default_value, 0.0, 100.0, syncable).unwrap()
}

fn modifier(id: u128, amount: f64, operation: Operation) -> AttributeModifier {
    AttributeModifier::new(key(id), amount, operation)
}

fn key(id: u128) -> Identifier {
    Identifier::parse(&format!("test:modifier_{id:x}")).unwrap()
}

fn instance(definition: AttributeDefinition, capacity: usize) -> AttributeInstance {
    AttributeInstance::try_new(definition, capacity).unwrap()
}

#[test]
fn identifier_round_trips_vanilla_namespace_path_and_java_hash() {
    let defaulted = Identifier::parse("generic.attack_damage").unwrap();
    assert_eq!(defaulted.namespace(), "minecraft");
    assert_eq!(defaulted.path(), "generic.attack_damage");
    assert_eq!(defaulted.as_str(), "minecraft:generic.attack_damage");

    let custom = Identifier::parse("example.mod:path/to-value_2").unwrap();
    assert_eq!(Identifier::parse(custom.as_str()).unwrap(), custom);
    assert_eq!(custom.namespace(), "example.mod");
    assert_eq!(custom.path(), "path/to-value_2");
    assert_eq!(custom.java_hash_code(), -1_310_731_522);

    let explicit_empty_namespace = Identifier::from_namespace_and_path("", "path").unwrap();
    assert_eq!(explicit_empty_namespace.as_str(), ":path");
    assert_eq!(explicit_empty_namespace.namespace(), "");
    assert_eq!(Identifier::parse(":path").unwrap().namespace(), "minecraft");

    assert_eq!(
        Identifier::parse("Minecraft:bad").unwrap_err(),
        IdentifierError::InvalidNamespace
    );
    assert_eq!(
        Identifier::parse("minecraft:bad:colon").unwrap_err(),
        IdentifierError::InvalidPath
    );
    assert_eq!(
        Identifier::from_namespace_and_path("..", "value").unwrap_err(),
        IdentifierError::InvalidNamespace
    );

    let oversized = format!("test:{}", "a".repeat(MAX_IDENTIFIER_BYTES));
    assert_eq!(
        Identifier::parse(&oversized).unwrap_err(),
        IdentifierError::TooLong {
            length: oversized.len(),
            maximum: MAX_IDENTIFIER_BYTES,
        }
    );

    let maximum = format!("a:{}", "a".repeat(MAX_IDENTIFIER_BYTES - 2));
    assert_eq!(Identifier::parse(&maximum).unwrap().as_str(), maximum);
}

#[test]
fn ranged_definition_validation_and_sanitization_match_java() {
    assert_eq!(
        AttributeDefinition::ranged(HEALTH, 1.0, 2.0, 1.0, false).unwrap_err(),
        AttributeDefinitionError::MinimumAboveMaximum
    );
    assert_eq!(
        AttributeDefinition::ranged(HEALTH, -1.0, 0.0, 2.0, false).unwrap_err(),
        AttributeDefinitionError::DefaultBelowMinimum
    );
    assert_eq!(
        AttributeDefinition::ranged(HEALTH, 3.0, 0.0, 2.0, false).unwrap_err(),
        AttributeDefinitionError::DefaultAboveMaximum
    );

    let definition = AttributeDefinition::ranged(HEALTH, 1.0, -2.0, 4.0, false).unwrap();
    assert_eq!(definition.sanitize(f64::NEG_INFINITY), -2.0);
    assert_eq!(definition.sanitize(-2.0), -2.0);
    assert_eq!(definition.sanitize(3.0), 3.0);
    assert_eq!(definition.sanitize(f64::INFINITY), 4.0);
    assert_eq!(definition.sanitize(f64::NAN), -2.0);

    let unbounded = AttributeDefinition::unbounded(SPEED, 1.0, false);
    assert!(unbounded.sanitize(f64::NAN).is_nan());
    assert_eq!(unbounded.sanitize(f64::INFINITY), f64::INFINITY);
}

#[test]
fn base_assignment_uses_java_inequality_for_nan_and_signed_zero() {
    let definition = AttributeDefinition::ranged(HEALTH, -0.0, 0.0, 100.0, false).unwrap();
    let mut attribute = instance(definition, 0);
    assert_eq!(attribute.value().to_bits(), (-0.0_f64).to_bits());
    assert!(!attribute.is_value_dirty());

    let unchanged = attribute.set_base_value(0.0);
    assert_eq!(unchanged.notifications(), 0);
    assert_eq!(attribute.base_value().to_bits(), (-0.0_f64).to_bits());
    assert!(!attribute.is_value_dirty());

    let first_nan = attribute.set_base_value(f64::NAN);
    let second_nan = attribute.set_base_value(f64::NAN);
    assert_eq!(first_nan.notifications(), 1);
    assert_eq!(second_nan.notifications(), 1);
    assert!(attribute.is_value_dirty());
    assert_eq!(
        attribute.value(),
        0.0,
        "ranged NaN sanitizes to the minimum"
    );
}

#[test]
fn operation_phases_use_adjusted_base_and_clamp_only_the_final_result() {
    let mut attribute = instance(ranged(HEALTH, 4.0, false), 8);
    attribute
        .add_transient_modifier(modifier(1, 2.0, Operation::AddValue))
        .unwrap();
    attribute
        .add_transient_modifier(modifier(2, 0.5, Operation::AddMultipliedBase))
        .unwrap();
    attribute
        .add_transient_modifier(modifier(3, 0.25, Operation::AddMultipliedBase))
        .unwrap();
    attribute
        .add_transient_modifier(modifier(4, 0.5, Operation::AddMultipliedTotal))
        .unwrap();
    attribute
        .add_transient_modifier(modifier(5, -0.2, Operation::AddMultipliedTotal))
        .unwrap();

    assert_eq!(
        attribute.value().to_bits(),
        12.600000000000001_f64.to_bits()
    );

    let mut final_clamp = instance(ranged(SPEED, 100.0, false), 2);
    final_clamp
        .add_transient_modifier(modifier(1, 100.0, Operation::AddValue))
        .unwrap();
    final_clamp
        .add_transient_modifier(modifier(2, -0.75, Operation::AddMultipliedTotal))
        .unwrap();
    assert_eq!(final_clamp.value(), 50.0);
}

#[test]
fn fastutil_collision_iteration_matches_java_non_associative_addition_bits() {
    // Reproduced by oracle/FastutilAttributeOrderOracle.java against fastutil 8.5.18.
    let definition = AttributeDefinition::unbounded(HEALTH, 10_000_000_000_000_000.0, false);
    let one = modifier(2, 1.0, Operation::AddValue);
    let cancel = modifier(3, -10_000_000_000_000_000.0, Operation::AddValue);

    let mut forward = instance(definition, 2);
    forward.add_transient_modifier(one.clone()).unwrap();
    forward.add_transient_modifier(cancel.clone()).unwrap();

    let mut reverse = instance(definition, 2);
    reverse.add_transient_modifier(cancel).unwrap();
    reverse.add_transient_modifier(one).unwrap();

    assert_eq!(forward.value().to_bits(), 1.0_f64.to_bits());
    assert_eq!(reverse.value().to_bits(), 0.0_f64.to_bits());
}

#[test]
fn fastutil_wrapped_cluster_removal_matches_java_non_associative_bits() {
    let definition = AttributeDefinition::unbounded(HEALTH, 10_000_000_000_000_000.0, false);
    let mut attribute = instance(definition, 3);
    attribute
        .add_transient_modifier(modifier(21, 0.0, Operation::AddValue))
        .unwrap();
    attribute
        .add_transient_modifier(modifier(39, -10_000_000_000_000_000.0, Operation::AddValue))
        .unwrap();
    attribute
        .add_transient_modifier(modifier(2, 1.0, Operation::AddValue))
        .unwrap();

    assert_eq!(attribute.value().to_bits(), 0.0_f64.to_bits());
    assert_eq!(attribute.remove_modifier(&key(21)).notifications(), 1);
    assert_eq!(attribute.value().to_bits(), 1.0_f64.to_bits());
}

#[test]
fn fastutil_resize_matches_java_non_associative_bits_without_growing_storage() {
    let definition = AttributeDefinition::unbounded(HEALTH, 10_000_000_000_000_000.0, false);
    let mut attribute = instance(definition, 25);
    let capacities = attribute.capacities();
    for id in 0..24 {
        let amount = match id {
            2 => 1.0,
            3 => -10_000_000_000_000_000.0,
            _ => 0.0,
        };
        attribute
            .add_transient_modifier(modifier(id, amount, Operation::AddValue))
            .unwrap();
    }

    assert_eq!(attribute.value().to_bits(), 1.0_f64.to_bits());
    attribute
        .add_transient_modifier(modifier(24, 0.0, Operation::AddValue))
        .unwrap();
    assert_eq!(attribute.value().to_bits(), 0.0_f64.to_bits());

    for id in [0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15] {
        assert_eq!(attribute.remove_modifier(&key(id)).notifications(), 1);
    }
    assert_eq!(attribute.value().to_bits(), 1.0_f64.to_bits());
    assert_eq!(attribute.capacities(), capacities);
}

#[test]
fn replace_from_reinserts_fastutil_values_like_java_put_all() {
    let definition = AttributeDefinition::unbounded(HEALTH, 10_000_000_000_000_000.0, false);
    let mut source = instance(definition, 2);
    source
        .add_transient_modifier(modifier(2, 1.0, Operation::AddValue))
        .unwrap();
    source
        .add_transient_modifier(modifier(3, -10_000_000_000_000_000.0, Operation::AddValue))
        .unwrap();
    assert_eq!(source.value().to_bits(), 1.0_f64.to_bits());

    let mut target = instance(definition, 2);
    target.replace_from(&source).unwrap();
    assert_eq!(target.value().to_bits(), 0.0_f64.to_bits());
}

#[test]
fn strict_add_duplicate_and_capacity_failures_are_atomic() {
    let mut attribute = instance(ranged(HEALTH, 10.0, false), 1);
    let first = modifier(1, 2.0, Operation::AddValue);
    attribute.add_transient_modifier(first.clone()).unwrap();
    let before = attribute.value();

    assert_eq!(
        attribute.add_permanent_modifier(first.clone()).unwrap_err(),
        AttributeInstanceError::DuplicateModifier {
            id: first.id().clone()
        }
    );
    let second = modifier(2, 3.0, Operation::AddValue);
    assert_eq!(
        attribute
            .add_transient_modifier(second.clone())
            .unwrap_err(),
        AttributeInstanceError::ModifierCapacityExceeded { capacity: 1 }
    );
    assert_eq!(attribute.modifier(first.id()), Some(first));
    assert_eq!(attribute.modifier(second.id()), None);
    assert_eq!(attribute.value(), before);
}

#[test]
fn transient_upsert_uses_record_identity_before_value_equality() {
    let mut attribute = instance(ranged(HEALTH, 10.0, false), 1);
    let original = modifier(1, 2.0, Operation::AddValue);
    attribute.add_transient_modifier(original.clone()).unwrap();
    assert_eq!(attribute.value(), 12.0);

    let distinct_equal = modifier(1, 2.0, Operation::AddValue);
    let changed = attribute
        .add_or_update_transient_modifier(distinct_equal.clone())
        .unwrap();
    assert_eq!(changed.notifications(), 1);
    assert!(attribute.is_value_dirty());
    assert_eq!(attribute.value(), 12.0);

    let unchanged = attribute
        .add_or_update_transient_modifier(distinct_equal)
        .unwrap();
    assert_eq!(unchanged.notifications(), 0);
    assert!(!attribute.is_value_dirty());

    let replacement = modifier(1, 3.0, Operation::AddValue);
    let changed = attribute
        .add_or_update_transient_modifier(replacement)
        .unwrap();
    assert_eq!(changed.notifications(), 1);
    assert!(attribute.is_value_dirty());
    assert_eq!(attribute.value(), 13.0);
}

#[test]
fn cross_operation_upsert_and_remove_preserve_java_bucket_residue() {
    let mut attribute = instance(AttributeDefinition::unbounded(HEALTH, 10.0, false), 1);
    let additive = modifier(1, 2.0, Operation::AddValue);
    let total = modifier(1, 0.5, Operation::AddMultipliedTotal);

    attribute.add_transient_modifier(additive).unwrap();
    attribute
        .add_or_update_transient_modifier(total.clone())
        .unwrap();
    assert_eq!(attribute.modifier(total.id()), Some(total.clone()));
    assert_eq!(attribute.value(), 18.0);

    let removed = attribute.remove_modifier(total.id());
    assert_eq!(removed.notifications(), 1);
    assert!(!attribute.has_modifier(total.id()));
    assert_eq!(attribute.value(), 12.0);
    assert_eq!(attribute.remove_modifier(total.id()).notifications(), 0);
}

#[test]
fn stale_operation_slots_remain_bounded_and_failed_add_is_atomic() {
    let mut attribute = instance(AttributeDefinition::unbounded(HEALTH, 10.0, false), 2);
    let stale = modifier(1, 1.0, Operation::AddValue);
    attribute.add_transient_modifier(stale.clone()).unwrap();
    attribute
        .add_or_update_transient_modifier(modifier(1, 0.0, Operation::AddMultipliedTotal))
        .unwrap();
    attribute.remove_modifier(stale.id());
    let live = modifier(2, 2.0, Operation::AddValue);
    attribute.add_transient_modifier(live).unwrap();
    let before = attribute.value();

    let blocked = modifier(3, 4.0, Operation::AddValue);
    assert_eq!(
        attribute
            .add_transient_modifier(blocked.clone())
            .unwrap_err(),
        AttributeInstanceError::OperationCapacityExceeded {
            operation: Operation::AddValue,
            capacity: 2,
        }
    );
    assert_eq!(attribute.modifier(blocked.id()), None);
    assert_eq!(attribute.value(), before);
}

#[test]
fn permanent_replace_reports_remove_and_add_notifications() {
    let mut attribute = instance(ranged(HEALTH, 10.0, false), 1);
    let first = modifier(1, 2.0, Operation::AddValue);
    assert_eq!(
        attribute
            .add_or_replace_permanent_modifier(first)
            .unwrap()
            .notifications(),
        1
    );
    let replacement = modifier(1, 0.5, Operation::AddMultipliedTotal);
    assert_eq!(
        attribute
            .add_or_replace_permanent_modifier(replacement.clone())
            .unwrap()
            .notifications(),
        2
    );
    assert_eq!(attribute.value(), 15.0);
    assert_eq!(
        attribute.permanent_modifiers().collect::<Vec<_>>(),
        vec![replacement]
    );
}

#[test]
fn save_projects_only_permanent_payload_even_after_transient_shadowing() {
    let mut attribute = instance(ranged(HEALTH, 10.0, false), 2);
    let saved = modifier(2, 2.0, Operation::AddValue);
    let transient = modifier(1, 3.0, Operation::AddValue);
    attribute.add_permanent_modifier(saved.clone()).unwrap();
    attribute.add_transient_modifier(transient).unwrap();

    let shadow = modifier(2, 0.5, Operation::AddMultipliedTotal);
    attribute
        .add_or_update_transient_modifier(shadow.clone())
        .unwrap();
    assert_eq!(attribute.modifier(saved.id()), Some(shadow));

    let packed = attribute.try_pack().unwrap();
    assert_eq!(packed.attribute, HEALTH);
    assert_eq!(packed.base_value, 10.0);
    assert_eq!(packed.modifiers, vec![saved]);
}

#[test]
fn load_replaces_base_and_overlays_permanent_modifiers_without_clearing() {
    let mut attribute = instance(AttributeDefinition::unbounded(HEALTH, 10.0, false), 4);
    let transient = modifier(1, 1.0, Operation::AddValue);
    let old_permanent = modifier(2, 2.0, Operation::AddValue);
    attribute.add_transient_modifier(transient.clone()).unwrap();
    attribute
        .add_permanent_modifier(old_permanent.clone())
        .unwrap();
    attribute.value();

    let loaded_replacement = modifier(2, 0.5, Operation::AddMultipliedTotal);
    let loaded_new = modifier(3, 3.0, Operation::AddValue);
    let packed = PackedAttribute {
        attribute: HEALTH,
        base_value: 20.0,
        modifiers: vec![loaded_new.clone(), loaded_replacement.clone()],
    };
    let dirty = attribute.apply_packed(&packed).unwrap();

    assert_eq!(dirty.notifications(), 1);
    assert_eq!(attribute.base_value(), 20.0);
    assert_eq!(attribute.modifier(transient.id()), Some(transient.clone()));
    assert_eq!(
        attribute.modifier(old_permanent.id()),
        Some(loaded_replacement.clone())
    );
    assert_eq!(attribute.value(), 39.0);
    assert_eq!(
        attribute.try_pack().unwrap().modifiers,
        vec![loaded_replacement, loaded_new]
    );
}

#[test]
fn load_mismatch_and_capacity_failure_leave_instance_unchanged() {
    let mut attribute = instance(ranged(HEALTH, 10.0, false), 1);
    let existing = modifier(1, 2.0, Operation::AddValue);
    attribute.add_transient_modifier(existing.clone()).unwrap();
    let before_value = attribute.value();

    let mismatch = PackedAttribute {
        attribute: SPEED,
        base_value: 20.0,
        modifiers: Vec::new(),
    };
    assert_eq!(
        attribute.apply_packed(&mismatch).unwrap_err(),
        AttributeInstanceError::PackedAttributeMismatch {
            expected: HEALTH,
            actual: SPEED,
        }
    );

    let over_capacity = PackedAttribute {
        attribute: HEALTH,
        base_value: 30.0,
        modifiers: vec![modifier(2, 4.0, Operation::AddValue)],
    };
    assert_eq!(
        attribute.apply_packed(&over_capacity).unwrap_err(),
        AttributeInstanceError::ModifierCapacityExceeded { capacity: 1 }
    );
    assert_eq!(attribute.base_value(), 10.0);
    assert_eq!(attribute.modifier(existing.id()), Some(existing));
    assert_eq!(attribute.value(), before_value);
}

#[test]
fn replace_from_rejects_other_attributes_and_dirties_identical_copies() {
    let mut target = instance(ranged(HEALTH, 10.0, false), 0);
    let other_attribute = instance(ranged(SPEED, 1.0, false), 0);
    target.value();
    assert_eq!(
        target.replace_from(&other_attribute).unwrap_err(),
        AttributeInstanceError::SourceAttributeMismatch {
            expected: HEALTH,
            actual: SPEED,
        }
    );
    assert!(!target.is_value_dirty());
    assert_eq!(target.base_value(), 10.0);

    let identical = instance(ranged(HEALTH, 10.0, false), 0);
    assert_eq!(target.replace_from(&identical).unwrap().notifications(), 1);
    assert!(target.is_value_dirty());
}

#[test]
fn applying_an_empty_identical_projection_still_marks_the_instance_dirty() {
    let mut attribute = instance(ranged(HEALTH, 10.0, false), 0);
    attribute.value();
    let packed = PackedAttribute {
        attribute: HEALTH,
        base_value: 10.0,
        modifiers: Vec::new(),
    };

    assert_eq!(attribute.apply_packed(&packed).unwrap().notifications(), 1);
    assert!(attribute.is_value_dirty());
}

#[test]
fn duplicate_loaded_ids_use_last_payload_and_keep_each_java_operation_bucket() {
    let mut attribute = instance(AttributeDefinition::unbounded(HEALTH, 10.0, false), 2);
    let id = key(1);
    let packed = PackedAttribute {
        attribute: HEALTH,
        base_value: 10.0,
        modifiers: vec![
            AttributeModifier::new(id.clone(), 2.0, Operation::AddValue),
            AttributeModifier::new(id.clone(), 0.5, Operation::AddMultipliedTotal),
        ],
    };
    attribute.apply_packed(&packed).unwrap();

    assert_eq!(
        attribute.modifier(&id),
        Some(AttributeModifier::new(
            id.clone(),
            0.5,
            Operation::AddMultipliedTotal
        ))
    );
    assert_eq!(attribute.value(), 18.0);
    assert_eq!(
        attribute.try_pack().unwrap().modifiers,
        vec![AttributeModifier::new(
            id.clone(),
            0.5,
            Operation::AddMultipliedTotal
        )]
    );
}

#[test]
fn remove_all_targets_canonical_ids_and_leaves_only_unreachable_residue() {
    let mut attribute = instance(AttributeDefinition::unbounded(HEALTH, 10.0, false), 2);
    attribute
        .add_transient_modifier(modifier(1, 2.0, Operation::AddValue))
        .unwrap();
    attribute
        .add_or_update_transient_modifier(modifier(1, 0.5, Operation::AddMultipliedTotal))
        .unwrap();
    attribute
        .add_permanent_modifier(modifier(2, 1.0, Operation::AddValue))
        .unwrap();

    assert_eq!(attribute.remove_all_modifiers().notifications(), 2);
    assert_eq!(attribute.modifiers().count(), 0);
    assert_eq!(attribute.permanent_modifiers().count(), 0);
    assert_eq!(attribute.value(), 12.0);
    assert_eq!(attribute.remove_all_modifiers().notifications(), 0);
}

#[test]
fn instance_hard_limit_is_explicit() {
    assert_eq!(
        AttributeInstance::try_new(ranged(HEALTH, 10.0, false), MAX_MODIFIERS_PER_ATTRIBUTE + 1)
            .unwrap_err(),
        AttributeInstanceError::CapacityExceedsHardLimit {
            requested: MAX_MODIFIERS_PER_ATTRIBUTE + 1,
            maximum: MAX_MODIFIERS_PER_ATTRIBUTE,
        }
    );
}

#[test]
fn map_reads_supplier_values_without_instantiation_or_publication() {
    let template_modifiers = [TemplateModifier::permanent(modifier(
        1,
        2.0,
        Operation::AddValue,
    ))];
    let templates = [AttributeTemplate::new(
        ranged(HEALTH, 10.0, true),
        20.0,
        &template_modifiers,
    )];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(1, 2).unwrap()).unwrap();

    assert!(attributes.has_attribute(HEALTH));
    assert!(attributes.has_modifier(HEALTH, &key(1)));
    assert_eq!(attributes.get_base_value(HEALTH).unwrap(), 20.0);
    assert_eq!(attributes.get_value(HEALTH).unwrap(), 22.0);
    assert_eq!(attributes.get_modifier_value(HEALTH, &key(1)).unwrap(), 2.0);
    assert_eq!(attributes.instantiated_len(), 0);
    assert!(attributes.pending_updates().is_empty());
    assert!(attributes.pending_syncs().is_empty());

    assert_eq!(
        attributes.get_value(UNKNOWN).unwrap_err(),
        AttributeMapError::UnknownAttribute { attribute: UNKNOWN }
    );
    assert_eq!(
        attributes.get_modifier_value(HEALTH, &key(9)).unwrap_err(),
        AttributeMapError::UnknownModifier {
            attribute: HEALTH,
            id: key(9),
        }
    );
}

#[test]
fn lazy_instantiation_and_mutation_publish_semantic_ordered_deduplicated_facts() {
    let templates = [
        AttributeTemplate::without_modifiers(ranged(SPEED, 1.0, false), 1.0),
        AttributeTemplate::without_modifiers(ranged(HEALTH, 10.0, true), 10.0),
    ];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(2, 2).unwrap()).unwrap();

    assert_eq!(
        attributes.ensure_instance(SPEED).unwrap(),
        InstantiationOutcome::Created
    );
    assert_eq!(
        attributes.ensure_instance(HEALTH).unwrap(),
        InstantiationOutcome::Created
    );
    assert_eq!(attributes.pending_updates(), &[HEALTH, SPEED]);
    assert_eq!(attributes.pending_syncs(), &[HEALTH]);
    assert_eq!(
        attributes.ensure_instance(HEALTH).unwrap(),
        InstantiationOutcome::Existing
    );

    attributes.clear_pending_updates();
    attributes.clear_pending_syncs();
    assert_eq!(
        attributes
            .set_base_value(HEALTH, 10.0)
            .unwrap()
            .notifications(),
        0
    );
    assert!(attributes.pending_updates().is_empty());

    attributes.set_base_value(HEALTH, 20.0).unwrap();
    attributes
        .add_transient_modifier(HEALTH, modifier(1, 2.0, Operation::AddValue))
        .unwrap();
    assert_eq!(attributes.pending_updates(), &[HEALTH]);
    assert_eq!(attributes.pending_syncs(), &[HEALTH]);
    assert_eq!(attributes.get_value(HEALTH).unwrap(), 22.0);
    assert!(!attributes.instance(HEALTH).unwrap().is_value_dirty());
    assert_eq!(attributes.pending_updates(), &[HEALTH]);
    assert_eq!(attributes.pending_syncs(), &[HEALTH]);

    attributes.clear_pending_updates();
    assert!(attributes.pending_updates().is_empty());
    assert_eq!(attributes.pending_syncs(), &[HEALTH]);
    attributes.clear_pending_syncs();
    assert!(attributes.pending_syncs().is_empty());
}

#[test]
fn map_capacity_and_unknown_mutation_fail_without_partial_state() {
    let templates = [
        AttributeTemplate::without_modifiers(ranged(HEALTH, 10.0, true), 10.0),
        AttributeTemplate::without_modifiers(ranged(SPEED, 1.0, false), 1.0),
    ];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(1, 1).unwrap()).unwrap();
    attributes.ensure_instance(HEALTH).unwrap();
    attributes.clear_pending_updates();
    attributes.clear_pending_syncs();

    assert_eq!(
        attributes.ensure_instance(SPEED).unwrap_err(),
        AttributeMapError::InstanceCapacityExceeded { capacity: 1 }
    );
    assert_eq!(attributes.instantiated_len(), 1);
    assert!(attributes.instance(SPEED).is_none());
    assert!(attributes.pending_updates().is_empty());
    assert_eq!(
        attributes
            .add_transient_modifier(UNKNOWN, modifier(1, 1.0, Operation::AddValue))
            .unwrap_err(),
        AttributeMapError::UnknownAttribute { attribute: UNKNOWN }
    );
    assert_eq!(attributes.instantiated_len(), 1);
}

#[test]
fn strict_duplicate_after_lazy_lookup_keeps_java_instantiation_publication() {
    let existing = modifier(1, 2.0, Operation::AddValue);
    let template_modifiers = [TemplateModifier::permanent(existing.clone())];
    let templates = [AttributeTemplate::new(
        ranged(HEALTH, 10.0, true),
        10.0,
        &template_modifiers,
    )];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(1, 1).unwrap()).unwrap();

    assert_eq!(
        attributes
            .add_transient_modifier(HEALTH, existing.clone())
            .unwrap_err(),
        AttributeMapError::Instance {
            attribute: HEALTH,
            source: AttributeInstanceError::DuplicateModifier {
                id: existing.id().clone(),
            },
        }
    );
    assert_eq!(attributes.instantiated_len(), 1);
    assert_eq!(attributes.pending_updates(), &[HEALTH]);
    assert_eq!(attributes.pending_syncs(), &[HEALTH]);
    assert_eq!(attributes.get_value(HEALTH).unwrap(), 12.0);
}

#[test]
fn map_remove_does_not_materialize_a_supplier_only_instance() {
    let templates = [AttributeTemplate::without_modifiers(
        ranged(HEALTH, 10.0, true),
        10.0,
    )];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(1, 1).unwrap()).unwrap();

    assert_eq!(
        attributes
            .remove_modifier(HEALTH, &key(1))
            .unwrap()
            .notifications(),
        0
    );
    assert_eq!(attributes.instantiated_len(), 0);
    assert!(attributes.pending_updates().is_empty());
}

#[test]
fn map_transient_replace_removes_old_permanence_before_adding() {
    let template_modifiers = [TemplateModifier::permanent(modifier(
        1,
        2.0,
        Operation::AddValue,
    ))];
    let templates = [AttributeTemplate::new(
        ranged(HEALTH, 10.0, true),
        10.0,
        &template_modifiers,
    )];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(1, 1).unwrap()).unwrap();
    attributes.ensure_instance(HEALTH).unwrap();
    attributes.clear_pending_updates();
    attributes.clear_pending_syncs();

    let replacement = modifier(1, 0.5, Operation::AddMultipliedTotal);
    let effect = attributes
        .replace_transient_modifier(HEALTH, replacement)
        .unwrap();
    assert_eq!(effect.notifications(), 2);
    assert_eq!(attributes.get_value(HEALTH).unwrap(), 15.0);
    assert!(
        attributes
            .instance(HEALTH)
            .unwrap()
            .permanent_modifiers()
            .next()
            .is_none()
    );
    assert!(attributes.try_pack().unwrap()[0].modifiers.is_empty());
    assert_eq!(attributes.pending_updates(), &[HEALTH]);
    assert_eq!(attributes.pending_syncs(), &[HEALTH]);
}

#[test]
fn reset_base_uses_supplier_base_and_preserves_lazy_instances() {
    let templates = [
        AttributeTemplate::without_modifiers(ranged(HEALTH, 10.0, true), 20.0),
        AttributeTemplate::without_modifiers(ranged(SPEED, 1.0, false), 2.0),
    ];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(2, 0).unwrap()).unwrap();

    assert!(attributes.reset_base_value(SPEED));
    assert!(attributes.instance(SPEED).is_none());
    assert!(!attributes.reset_base_value(UNKNOWN));

    attributes.set_base_value(HEALTH, 30.0).unwrap();
    attributes.clear_pending_updates();
    attributes.clear_pending_syncs();
    assert!(attributes.reset_base_value(HEALTH));
    assert_eq!(attributes.get_base_value(HEALTH).unwrap(), 20.0);
    assert_eq!(attributes.pending_updates(), &[HEALTH]);
    assert_eq!(attributes.pending_syncs(), &[HEALTH]);
}

#[test]
fn map_pack_uses_semantic_attribute_order_and_permanent_insertion_order() {
    let templates = [
        AttributeTemplate::without_modifiers(ranged(SPEED, 1.0, false), 1.0),
        AttributeTemplate::without_modifiers(ranged(HEALTH, 10.0, true), 10.0),
    ];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(2, 3).unwrap()).unwrap();
    attributes.ensure_instance(SPEED).unwrap();
    attributes.ensure_instance(HEALTH).unwrap();
    attributes
        .add_permanent_modifier(HEALTH, modifier(3, 3.0, Operation::AddValue))
        .unwrap();
    attributes
        .add_permanent_modifier(HEALTH, modifier(1, 1.0, Operation::AddValue))
        .unwrap();
    attributes
        .add_transient_modifier(HEALTH, modifier(2, 2.0, Operation::AddValue))
        .unwrap();

    let packed = attributes.try_pack().unwrap();
    assert_eq!(
        packed
            .iter()
            .map(|value| value.attribute)
            .collect::<Vec<_>>(),
        vec![HEALTH, SPEED]
    );
    assert_eq!(
        packed[0]
            .modifiers
            .iter()
            .map(|value| value.id().clone())
            .collect::<Vec<_>>(),
        vec![key(3), key(1)]
    );
    assert!(packed[1].modifiers.is_empty());
}

#[test]
fn map_apply_ignores_unknown_attributes_and_marks_known_syncable_rows() {
    let templates = [AttributeTemplate::without_modifiers(
        ranged(HEALTH, 10.0, true),
        10.0,
    )];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(1, 2).unwrap()).unwrap();
    let packed = [
        PackedAttribute {
            attribute: UNKNOWN,
            base_value: 99.0,
            modifiers: Vec::new(),
        },
        PackedAttribute {
            attribute: HEALTH,
            base_value: 20.0,
            modifiers: vec![modifier(1, 2.0, Operation::AddValue)],
        },
    ];

    assert_eq!(
        attributes.apply_packed(&packed).unwrap(),
        ApplyReport {
            applied: 1,
            ignored_unknown: 1,
        }
    );
    assert_eq!(attributes.get_value(HEALTH).unwrap(), 22.0);
    assert_eq!(attributes.pending_updates(), &[HEALTH]);
    assert_eq!(attributes.pending_syncs(), &[HEALTH]);
}

#[test]
fn map_rejects_oversized_load_record_before_lazy_instantiation() {
    let templates = [AttributeTemplate::without_modifiers(
        ranged(HEALTH, 10.0, true),
        10.0,
    )];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(1, 1).unwrap()).unwrap();
    let packed = [PackedAttribute {
        attribute: HEALTH,
        base_value: 20.0,
        modifiers: vec![
            modifier(1, 1.0, Operation::AddValue),
            modifier(2, 2.0, Operation::AddValue),
        ],
    }];

    assert_eq!(
        attributes.apply_packed(&packed).unwrap_err(),
        AttributeMapError::Instance {
            attribute: HEALTH,
            source: AttributeInstanceError::ModifierCapacityExceeded { capacity: 1 },
        }
    );
    assert_eq!(attributes.instantiated_len(), 0);
    assert!(attributes.pending_updates().is_empty());
    assert!(attributes.pending_syncs().is_empty());
}

#[test]
fn map_load_keeps_accepted_prefix_when_a_later_record_exceeds_capacity() {
    let templates = [AttributeTemplate::without_modifiers(
        ranged(HEALTH, 10.0, true),
        10.0,
    )];
    let mut attributes =
        AttributeMap::try_new(&templates, AttributeMapLimits::new(1, 1).unwrap()).unwrap();
    let accepted = modifier(1, 2.0, Operation::AddValue);
    let packed = [
        PackedAttribute {
            attribute: HEALTH,
            base_value: 20.0,
            modifiers: vec![accepted.clone()],
        },
        PackedAttribute {
            attribute: HEALTH,
            base_value: 30.0,
            modifiers: vec![modifier(2, 3.0, Operation::AddValue)],
        },
    ];

    assert_eq!(
        attributes.apply_packed(&packed).unwrap_err(),
        AttributeMapError::Instance {
            attribute: HEALTH,
            source: AttributeInstanceError::ModifierCapacityExceeded { capacity: 1 },
        }
    );
    assert_eq!(attributes.get_base_value(HEALTH).unwrap(), 20.0);
    assert_eq!(attributes.get_value(HEALTH).unwrap(), 22.0);
    assert_eq!(attributes.try_pack().unwrap()[0].modifiers, vec![accepted]);
}

#[test]
fn assign_variants_follow_instantiated_source_projection() {
    let templates = [
        AttributeTemplate::without_modifiers(ranged(HEALTH, 10.0, true), 10.0),
        AttributeTemplate::without_modifiers(ranged(SPEED, 1.0, false), 1.0),
    ];
    let limits = AttributeMapLimits::new(2, 3).unwrap();
    let mut source = AttributeMap::try_new(&templates, limits).unwrap();
    source.set_base_value(HEALTH, 30.0).unwrap();
    source
        .add_permanent_modifier(HEALTH, modifier(1, 2.0, Operation::AddValue))
        .unwrap();
    source
        .add_transient_modifier(HEALTH, modifier(2, 0.5, Operation::AddMultipliedTotal))
        .unwrap();

    let mut bases = AttributeMap::try_new(&templates, limits).unwrap();
    bases.assign_base_values(&source).unwrap();
    assert_eq!(bases.get_base_value(HEALTH).unwrap(), 30.0);
    assert!(!bases.has_modifier(HEALTH, &key(1)));
    assert!(bases.instance(SPEED).is_none());

    let mut permanents = AttributeMap::try_new(&templates, limits).unwrap();
    permanents.assign_permanent_modifiers(&source).unwrap();
    assert!(permanents.has_modifier(HEALTH, &key(1)));
    assert!(!permanents.has_modifier(HEALTH, &key(2)));

    let mut all = AttributeMap::try_new(&templates, limits).unwrap();
    all.assign_all_values(&source).unwrap();
    assert_eq!(all.get_base_value(HEALTH).unwrap(), 30.0);
    assert!(all.has_modifier(HEALTH, &key(1)));
    assert!(all.has_modifier(HEALTH, &key(2)));
    assert_eq!(all.get_value(HEALTH).unwrap(), 48.0);
    assert!(all.instance(SPEED).is_none());
}

#[test]
fn permanent_assignment_retains_accepted_prefix_and_publication_on_later_duplicate() {
    let templates = [AttributeTemplate::without_modifiers(
        ranged(HEALTH, 10.0, true),
        10.0,
    )];
    let limits = AttributeMapLimits::new(1, 3).unwrap();
    let prefix = modifier(1, 1.0, Operation::AddValue);
    let duplicate = modifier(2, 2.0, Operation::AddValue);

    let mut source = AttributeMap::try_new(&templates, limits).unwrap();
    source
        .add_permanent_modifier(HEALTH, prefix.clone())
        .unwrap();
    source
        .add_permanent_modifier(HEALTH, duplicate.clone())
        .unwrap();

    let mut target = AttributeMap::try_new(&templates, limits).unwrap();
    target
        .add_permanent_modifier(HEALTH, duplicate.clone())
        .unwrap();
    target.clear_pending_updates();
    target.clear_pending_syncs();

    assert_eq!(
        target.assign_permanent_modifiers(&source).unwrap_err(),
        AttributeMapError::Instance {
            attribute: HEALTH,
            source: AttributeInstanceError::DuplicateModifier {
                id: duplicate.id().clone(),
            },
        }
    );
    assert_eq!(target.pending_updates(), &[HEALTH]);
    assert_eq!(target.pending_syncs(), &[HEALTH]);
    assert_eq!(target.get_value(HEALTH).unwrap(), 13.0);
    assert_eq!(
        target.try_pack().unwrap()[0].modifiers,
        vec![duplicate, prefix]
    );
}

#[test]
fn cross_attribute_semantic_order_is_sorted_while_assignment_retains_input_prefix() {
    let templates = [
        AttributeTemplate::without_modifiers(ranged(INPUT_FAILURE, 10.0, true), 10.0),
        AttributeTemplate::without_modifiers(ranged(INPUT_PREFIX, 10.0, true), 10.0),
    ];
    let limits = AttributeMapLimits::new(2, 2).unwrap();
    let duplicate = modifier(1, 2.0, Operation::AddValue);
    let prefix = modifier(2, 3.0, Operation::AddValue);

    let mut source = AttributeMap::try_new(&templates, limits).unwrap();
    source
        .add_permanent_modifier(INPUT_PREFIX, prefix.clone())
        .unwrap();
    source
        .add_permanent_modifier(INPUT_FAILURE, duplicate.clone())
        .unwrap();

    assert_eq!(source.pending_updates(), &[INPUT_FAILURE, INPUT_PREFIX]);
    assert_eq!(source.pending_syncs(), &[INPUT_FAILURE, INPUT_PREFIX]);
    assert_eq!(
        source.syncable_instances().collect::<Vec<_>>(),
        vec![INPUT_FAILURE, INPUT_PREFIX]
    );
    assert_eq!(
        source
            .try_pack()
            .unwrap()
            .iter()
            .map(|packed| packed.attribute)
            .collect::<Vec<_>>(),
        vec![INPUT_FAILURE, INPUT_PREFIX]
    );

    let mut target = AttributeMap::try_new(&templates, limits).unwrap();
    target
        .add_permanent_modifier(INPUT_FAILURE, duplicate.clone())
        .unwrap();
    target.clear_pending_updates();
    target.clear_pending_syncs();

    assert_eq!(
        target.assign_permanent_modifiers(&source).unwrap_err(),
        AttributeMapError::Instance {
            attribute: INPUT_FAILURE,
            source: AttributeInstanceError::DuplicateModifier {
                id: duplicate.id().clone(),
            },
        }
    );
    assert!(target.has_modifier(INPUT_PREFIX, prefix.id()));
    assert_eq!(target.pending_updates(), &[INPUT_PREFIX]);
    assert_eq!(target.pending_syncs(), &[INPUT_PREFIX]);
}

#[test]
fn permanent_assignment_reuses_matching_stale_operation_slot_at_capacity() {
    let templates = [AttributeTemplate::without_modifiers(
        AttributeDefinition::unbounded(HEALTH, 10.0, false),
        10.0,
    )];
    let limits = AttributeMapLimits::new(1, 1).unwrap();
    let id = key(1);

    let mut target = AttributeMap::try_new(&templates, limits).unwrap();
    target
        .add_transient_modifier(
            HEALTH,
            AttributeModifier::new(id.clone(), 1.0, Operation::AddValue),
        )
        .unwrap();
    target
        .add_or_update_transient_modifier(
            HEALTH,
            AttributeModifier::new(id.clone(), 0.0, Operation::AddMultipliedTotal),
        )
        .unwrap();
    target.remove_modifier(HEALTH, &id).unwrap();
    assert_eq!(target.get_value(HEALTH).unwrap(), 11.0);

    let mut source = AttributeMap::try_new(&templates, limits).unwrap();
    let permanent = AttributeModifier::new(id, 2.0, Operation::AddValue);
    source
        .add_permanent_modifier(HEALTH, permanent.clone())
        .unwrap();

    target.assign_permanent_modifiers(&source).unwrap();
    assert_eq!(target.get_value(HEALTH).unwrap(), 12.0);
    assert_eq!(target.try_pack().unwrap()[0].modifiers, vec![permanent]);
}

#[test]
fn map_limit_and_template_validation_cover_constructor_failures_and_keep_last() {
    assert_eq!(
        AttributeMapLimits::new(MAX_ATTRIBUTE_INSTANCES + 1, 0).unwrap_err(),
        AttributeMapInitError::InstanceCapacityExceedsHardLimit {
            requested: MAX_ATTRIBUTE_INSTANCES + 1,
            maximum: MAX_ATTRIBUTE_INSTANCES,
        }
    );
    assert_eq!(
        AttributeMapLimits::new(0, MAX_MODIFIERS_PER_ATTRIBUTE + 1).unwrap_err(),
        AttributeMapInitError::ModifierCapacityExceedsHardLimit {
            requested: MAX_MODIFIERS_PER_ATTRIBUTE + 1,
            maximum: MAX_MODIFIERS_PER_ATTRIBUTE,
        }
    );

    let template = AttributeTemplate::without_modifiers(ranged(HEALTH, 10.0, false), 10.0);
    let too_many_templates = vec![template; MAX_ATTRIBUTE_INSTANCES + 1];
    assert_eq!(
        AttributeMap::try_new(&too_many_templates, AttributeMapLimits::new(0, 0).unwrap())
            .unwrap_err(),
        AttributeMapInitError::TooManyTemplates {
            count: MAX_ATTRIBUTE_INSTANCES + 1,
            maximum: MAX_ATTRIBUTE_INSTANCES,
        }
    );

    let duplicate_attributes = [
        AttributeTemplate::without_modifiers(ranged(HEALTH, 10.0, false), 10.0),
        AttributeTemplate::without_modifiers(ranged(HEALTH, 20.0, true), 20.0),
    ];
    let mut keep_last = AttributeMap::try_new(
        &duplicate_attributes,
        AttributeMapLimits::new(1, 0).unwrap(),
    )
    .unwrap();
    assert_eq!(keep_last.get_base_value(HEALTH).unwrap(), 20.0);
    keep_last.ensure_instance(HEALTH).unwrap();
    assert_eq!(keep_last.pending_syncs(), &[HEALTH]);

    let duplicate = modifier(1, 1.0, Operation::AddValue);
    let duplicate_modifiers = [
        TemplateModifier::transient(duplicate.clone()),
        TemplateModifier::permanent(duplicate.clone()),
    ];
    let bad_template = [AttributeTemplate::new(
        ranged(HEALTH, 10.0, false),
        10.0,
        &duplicate_modifiers,
    )];
    assert_eq!(
        AttributeMap::try_new(&bad_template, AttributeMapLimits::new(1, 2).unwrap()).unwrap_err(),
        AttributeMapInitError::Template {
            attribute: HEALTH,
            source: AttributeInstanceError::DuplicateModifier {
                id: duplicate.id().clone(),
            },
        }
    );

    let oversized_modifiers = [
        TemplateModifier::transient(modifier(1, 1.0, Operation::AddValue)),
        TemplateModifier::transient(modifier(2, 1.0, Operation::AddValue)),
    ];
    let oversized_template = [AttributeTemplate::new(
        ranged(HEALTH, 10.0, false),
        10.0,
        &oversized_modifiers,
    )];
    assert_eq!(
        AttributeMap::try_new(&oversized_template, AttributeMapLimits::new(1, 1).unwrap())
            .unwrap_err(),
        AttributeMapInitError::Template {
            attribute: HEALTH,
            source: AttributeInstanceError::ModifierCapacityExceeded { capacity: 1 },
        }
    );
}
