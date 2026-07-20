use super::*;

const ITEM_A: ItemKey = ItemKey::new(1).unwrap();
const ITEM_B: ItemKey = ItemKey::new(2).unwrap();
const COMPONENT_A: ComponentKey = ComponentKey::new(1).unwrap();
const COMPONENT_B: ComponentKey = ComponentKey::new(2).unwrap();

fn bytes(value: &[u8]) -> ComponentBytes {
    ComponentBytes::new(value).unwrap()
}

fn components(tag: u8) -> StackComponents {
    let mut components = StackComponents::new();
    components.set(COMPONENT_A, bytes(&[tag])).unwrap();
    components
}

fn stack(item: ItemKey, count: u32, tag: u8) -> ItemStackState {
    ItemStackState::occupied(item, count, components(tag)).unwrap()
}

fn damageable_stack(item: ItemKey, count: u32, damage: i32, max: i32) -> ItemStackState {
    let mut components = components(9);
    components.set_damage(Some(damage)).unwrap();
    components.set_max_damage(Some(max)).unwrap();
    ItemStackState::occupied(item, count, components).unwrap()
}

#[test]
fn slot_and_group_metadata_match_vanilla_order() {
    let expected = [
        (
            EquipmentSlot::MainHand,
            EquipmentSlotType::Hand,
            0,
            0,
            0,
            "mainhand",
        ),
        (
            EquipmentSlot::OffHand,
            EquipmentSlotType::Hand,
            1,
            0,
            5,
            "offhand",
        ),
        (
            EquipmentSlot::Feet,
            EquipmentSlotType::HumanoidArmor,
            0,
            1,
            1,
            "feet",
        ),
        (
            EquipmentSlot::Legs,
            EquipmentSlotType::HumanoidArmor,
            1,
            1,
            2,
            "legs",
        ),
        (
            EquipmentSlot::Chest,
            EquipmentSlotType::HumanoidArmor,
            2,
            1,
            3,
            "chest",
        ),
        (
            EquipmentSlot::Head,
            EquipmentSlotType::HumanoidArmor,
            3,
            1,
            4,
            "head",
        ),
        (
            EquipmentSlot::Body,
            EquipmentSlotType::AnimalArmor,
            0,
            1,
            6,
            "body",
        ),
        (
            EquipmentSlot::Saddle,
            EquipmentSlotType::Saddle,
            0,
            1,
            7,
            "saddle",
        ),
    ];
    for (ordinal, (slot, kind, index, limit, id, name)) in expected.into_iter().enumerate() {
        assert_eq!(EquipmentSlot::VALUES[ordinal], slot);
        assert_eq!(slot.slot_type(), kind);
        assert_eq!(slot.index(), index);
        assert_eq!(slot.count_limit(), limit);
        assert_eq!(slot.id(), id);
        assert_eq!(EquipmentSlot::by_id(id as i32), slot);
        assert_eq!(EquipmentSlot::by_name(name), Ok(slot));
    }
    assert_eq!(
        EquipmentSlotGroup::Armor.slots(),
        &[
            EquipmentSlot::Feet,
            EquipmentSlot::Legs,
            EquipmentSlot::Chest,
            EquipmentSlot::Head,
            EquipmentSlot::Body,
        ]
    );
    assert_eq!(
        EquipmentSlot::by_name("MAINHAND"),
        Err(EquipmentSlotNameError)
    );
    assert_eq!(
        EquipmentSlotGroup::by_name("ARMOR"),
        Err(EquipmentSlotGroupNameError)
    );
}

#[test]
fn components_use_one_global_bound_and_count_typed_fields() {
    let mut value = StackComponents::new();
    assert!(value.is_empty());
    value.set_damage(Some(1)).unwrap();
    value.set_max_damage(Some(10)).unwrap();
    value.set_unbreakable(true).unwrap();
    value.set(COMPONENT_A, bytes(&[1, 2, 3])).unwrap();
    assert_eq!(value.len(), 4);
    assert!(!value.is_empty());

    value.set_damage(None).unwrap();
    value.set_max_damage(None).unwrap();
    value.set_unbreakable(false).unwrap();
    value.remove(COMPONENT_A);
    assert!(value.is_empty());

    let oversized = vec![0; MAX_STACK_COMPONENT_SERIALIZED_BYTES + 1];
    assert!(matches!(
        ComponentBytes::new(&oversized),
        Err(ComponentError::UnsupportedStack { .. })
    ));
}

#[test]
fn aggregate_component_bound_rejects_without_partial_mutation() {
    let mut value = StackComponents::new();
    value
        .set(
            COMPONENT_A,
            bytes(&vec![0; MAX_STACK_COMPONENT_SERIALIZED_BYTES / 2]),
        )
        .unwrap();
    let before = value.clone();
    assert!(matches!(
        value.set(
            COMPONENT_B,
            bytes(&vec![0; MAX_STACK_COMPONENT_SERIALIZED_BYTES / 2]),
        ),
        Err(ComponentError::UnsupportedStack { .. })
    ));
    assert_eq!(value, before);
}

#[test]
fn matches_compares_exact_owned_components_and_count() {
    let left = stack(ITEM_A, 1, 7);
    assert!(left.matches(&stack(ITEM_A, 1, 7)));
    assert!(!left.matches(&stack(ITEM_A, 1, 8)));
    assert!(!left.matches(&stack(ITEM_B, 1, 7)));
    assert!(left.same_item_same_components(&stack(ITEM_A, 4, 7)));
    assert!(!left.matches(&stack(ITEM_A, 4, 7)));
}

#[test]
fn count_zero_then_one_resurrects_java_stack_identity() {
    let original = stack(ITEM_A, 2, 7);
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::Head, original.clone());
    let revision = equipment.revision(EquipmentSlot::Head);
    let mut mutation = StackMutation::new(revision);
    mutation.push(StackMutationOp::SetCount(0)).unwrap();
    mutation.push(StackMutationOp::SetCount(1)).unwrap();
    equipment
        .apply_inventory_tick_result(EquipmentSlot::Head, mutation)
        .unwrap();
    assert_eq!(equipment.get(EquipmentSlot::Head).item_key(), Some(ITEM_A));
    assert_eq!(equipment.get(EquipmentSlot::Head).count(), 1);
    assert_eq!(
        equipment
            .get(EquipmentSlot::Head)
            .components()
            .get(COMPONENT_A)
            .unwrap()
            .as_slice(),
        &[7]
    );
}

#[test]
fn inventory_tick_mutation_is_atomic_on_stale_or_unsupported_stack() {
    let mut equipment = EquipmentState::new();
    let original = stack(ITEM_A, 2, 1);
    equipment.set(EquipmentSlot::Head, original.clone());
    let revision = equipment.revision(EquipmentSlot::Head);

    let mut stale = StackMutation::new(SlotRevision::INITIAL);
    stale.push(StackMutationOp::SetCount(1)).unwrap();
    assert!(matches!(
        equipment.apply_inventory_tick_result(EquipmentSlot::Head, stale),
        Err(EquipmentMutationError::StaleRevision { .. })
    ));

    let mut unsupported = StackMutation::new(revision);
    unsupported.push(StackMutationOp::SetCount(1)).unwrap();
    unsupported
        .push(StackMutationOp::SetComponent {
            key: COMPONENT_B,
            value: bytes(&vec![0; MAX_STACK_COMPONENT_SERIALIZED_BYTES]),
        })
        .unwrap();
    assert!(matches!(
        equipment.apply_inventory_tick_result(EquipmentSlot::Head, unsupported),
        Err(EquipmentMutationError::Component(
            ComponentError::UnsupportedStack { .. }
        ))
    ));
    assert_eq!(equipment.get(EquipmentSlot::Head), &original);
}

#[test]
fn persistence_projects_owned_stacks_in_declaration_order() {
    let mut equipment = EquipmentState::new();
    let offhand = damageable_stack(ITEM_A, 1, 3, 10);
    let feet = stack(ITEM_B, 1, 2);
    equipment.set(EquipmentSlot::Feet, feet.clone());
    equipment.set(EquipmentSlot::OffHand, offhand.clone());
    equipment.set_guaranteed_drop(EquipmentSlot::Feet);
    let projection = equipment.persistence_projection();
    assert_eq!(projection.equipment().len(), 2);
    assert_eq!(
        projection.equipment().get(0).unwrap().slot,
        EquipmentSlot::OffHand
    );
    assert_eq!(projection.equipment().get(0).unwrap().value, offhand);
    assert_eq!(projection.equipment().get(1).unwrap().value, feet);
    assert_eq!(
        projection.drop_chances().get(0).unwrap().slot,
        EquipmentSlot::Feet
    );
}

fn replace(reason: ReplacementReason) -> ReplacementDecision {
    ReplacementDecision::Replace(reason)
}

fn pickup_facts(
    equipment: &EquipmentState,
    slot: EquipmentSlot,
    decision: ReplacementDecision,
) -> MobPickupFacts {
    MobPickupFacts {
        resolved: equipment.precondition(slot),
        fallback_main_hand: None,
        is_equippable_in_resolved_slot: true,
        resolved_replacement: decision,
        can_hold: true,
    }
}

#[test]
fn pickup_commit_atomically_mutates_both_owners_and_returns_drop_fact() {
    let mut equipment = EquipmentState::new();
    let old = stack(ITEM_B, 1, 2);
    equipment.set(EquipmentSlot::Head, old.clone());
    let mut source = PickupSource::new(PickupSourceId::new(7).unwrap(), stack(ITEM_A, 3, 1));
    let transaction = equipment
        .prepare_mob_pickup(
            &source,
            pickup_facts(
                &equipment,
                EquipmentSlot::Head,
                replace(ReplacementReason::BetterDefense),
            ),
            PickupRandomInput::ReplacementDrop(DropRoll::new(0.0).unwrap()),
        )
        .unwrap();
    let outcome = equipment
        .commit_mob_pickup(&mut source, transaction)
        .unwrap();
    assert_eq!(source.stack(), &stack(ITEM_A, 2, 1));
    assert_eq!(equipment.get(EquipmentSlot::Head), &stack(ITEM_A, 1, 1));
    assert_eq!(outcome.replaced_drop, Some(old));
    assert_eq!(equipment.drop_chance(EquipmentSlot::Head), 2.0);
    assert!(equipment.persistence_required());
}

#[test]
fn stale_replacement_facts_reject_before_pickup_plan() {
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::Head, stack(ITEM_B, 1, 1));
    let facts = pickup_facts(
        &equipment,
        EquipmentSlot::Head,
        replace(ReplacementReason::BetterDefense),
    );
    equipment.set(EquipmentSlot::Head, stack(ITEM_B, 1, 2));
    let source = PickupSource::new(PickupSourceId::new(1).unwrap(), stack(ITEM_A, 1, 1));
    assert!(matches!(
        equipment.prepare_mob_pickup(
            &source,
            facts,
            PickupRandomInput::ReplacementDrop(DropRoll::new(0.0).unwrap()),
        ),
        Err(PickupPrepareError::StaleReplacementFacts { .. })
    ));
}

#[test]
fn pickup_commit_rechecks_exact_source_and_equipment_snapshots() {
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::Head, stack(ITEM_B, 1, 1));
    let mut source = PickupSource::new(PickupSourceId::new(1).unwrap(), stack(ITEM_A, 2, 1));
    let transaction = equipment
        .prepare_mob_pickup(
            &source,
            pickup_facts(
                &equipment,
                EquipmentSlot::Head,
                replace(ReplacementReason::BetterDefense),
            ),
            PickupRandomInput::ReplacementDrop(DropRoll::new(0.9).unwrap()),
        )
        .unwrap();
    source.replace(stack(ITEM_A, 2, 9));
    let equipment_before = equipment.clone();
    assert!(matches!(
        equipment.commit_mob_pickup(&mut source, transaction),
        Err(PickupCommitError::StaleSource { .. })
    ));
    assert_eq!(equipment, equipment_before);

    let transaction = equipment
        .prepare_mob_pickup(
            &source,
            pickup_facts(
                &equipment,
                EquipmentSlot::Head,
                replace(ReplacementReason::BetterDefense),
            ),
            PickupRandomInput::ReplacementDrop(DropRoll::new(0.9).unwrap()),
        )
        .unwrap();
    equipment.set(EquipmentSlot::Head, stack(ITEM_B, 1, 8));
    let source_before = source.clone();
    assert!(matches!(
        equipment.commit_mob_pickup(&mut source, transaction),
        Err(PickupCommitError::StaleEquipment { .. })
    ));
    assert_eq!(source, source_before);
}

#[test]
fn armor_fallback_is_bound_to_exact_main_hand_precondition() {
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::Head, stack(ITEM_B, 1, 1));
    let head = equipment.precondition(EquipmentSlot::Head);
    let main = equipment.precondition(EquipmentSlot::MainHand);
    let mut source = PickupSource::new(PickupSourceId::new(1).unwrap(), stack(ITEM_A, 2, 1));
    let transaction = equipment
        .prepare_mob_pickup(
            &source,
            MobPickupFacts {
                resolved: head,
                fallback_main_hand: Some(main),
                is_equippable_in_resolved_slot: true,
                resolved_replacement: ReplacementDecision::Keep(ReplacementReason::WorseDefense),
                can_hold: true,
            },
            PickupRandomInput::None,
        )
        .unwrap();
    let outcome = equipment
        .commit_mob_pickup(&mut source, transaction)
        .unwrap();
    assert_eq!(outcome.slot, EquipmentSlot::MainHand);
    assert_eq!(equipment.main_hand(), &stack(ITEM_A, 2, 1));
}

#[test]
fn pickup_random_shape_and_all_rejections_are_typed() {
    let equipment = EquipmentState::new();
    let source = PickupSource::new(PickupSourceId::new(1).unwrap(), stack(ITEM_A, 1, 1));
    let facts = pickup_facts(
        &equipment,
        EquipmentSlot::MainHand,
        replace(ReplacementReason::EmptySlot),
    );
    assert_eq!(
        equipment.prepare_mob_pickup(
            &source,
            facts.clone(),
            PickupRandomInput::ReplacementDrop(DropRoll::new(0.0).unwrap()),
        ),
        Err(PickupPrepareError::UnexpectedReplacementDropRoll)
    );
    let empty = PickupSource::new(PickupSourceId::new(2).unwrap(), ItemStackState::EMPTY);
    assert_eq!(
        equipment.prepare_mob_pickup(&empty, facts, PickupRandomInput::None),
        Err(PickupPrepareError::EmptyStack)
    );
}

#[test]
fn pickup_rejects_missing_wrong_and_stale_fallback_facts() {
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::Head, stack(ITEM_B, 1, 1));
    let source = PickupSource::new(PickupSourceId::new(1).unwrap(), stack(ITEM_A, 1, 1));
    let head = equipment.precondition(EquipmentSlot::Head);
    let keep = ReplacementDecision::Keep(ReplacementReason::WorseDefense);

    assert_eq!(
        equipment.prepare_mob_pickup(
            &source,
            MobPickupFacts {
                resolved: head.clone(),
                fallback_main_hand: None,
                is_equippable_in_resolved_slot: true,
                resolved_replacement: keep,
                can_hold: true,
            },
            PickupRandomInput::None,
        ),
        Err(PickupPrepareError::MissingMainHandFallback)
    );
    assert_eq!(
        equipment.prepare_mob_pickup(
            &source,
            MobPickupFacts {
                resolved: head.clone(),
                fallback_main_hand: Some(head.clone()),
                is_equippable_in_resolved_slot: true,
                resolved_replacement: keep,
                can_hold: true,
            },
            PickupRandomInput::None,
        ),
        Err(PickupPrepareError::WrongMainHandFallbackSlot)
    );

    let stale_main = equipment.precondition(EquipmentSlot::MainHand);
    equipment.set(EquipmentSlot::MainHand, stack(ITEM_B, 1, 3));
    assert!(matches!(
        equipment.prepare_mob_pickup(
            &source,
            MobPickupFacts {
                resolved: head,
                fallback_main_hand: Some(stale_main),
                is_equippable_in_resolved_slot: true,
                resolved_replacement: keep,
                can_hold: true,
            },
            PickupRandomInput::None,
        ),
        Err(PickupPrepareError::StaleReplacementFacts {
            slot: EquipmentSlot::MainHand,
            ..
        })
    ));
}

#[test]
fn pickup_owner_rejections_and_wrong_source_do_not_mutate() {
    let mut equipment = EquipmentState::new();
    let source = PickupSource::new(PickupSourceId::new(1).unwrap(), stack(ITEM_A, 1, 1));
    let mut not_equippable = pickup_facts(
        &equipment,
        EquipmentSlot::MainHand,
        replace(ReplacementReason::EmptySlot),
    );
    not_equippable.is_equippable_in_resolved_slot = false;
    assert_eq!(
        equipment.prepare_mob_pickup(&source, not_equippable, PickupRandomInput::None),
        Err(PickupPrepareError::NotEquippable)
    );

    let mut cannot_hold = pickup_facts(
        &equipment,
        EquipmentSlot::MainHand,
        replace(ReplacementReason::EmptySlot),
    );
    cannot_hold.can_hold = false;
    assert_eq!(
        equipment.prepare_mob_pickup(&source, cannot_hold, PickupRandomInput::None),
        Err(PickupPrepareError::CannotHold)
    );

    let transaction = equipment
        .prepare_mob_pickup(
            &source,
            pickup_facts(
                &equipment,
                EquipmentSlot::MainHand,
                replace(ReplacementReason::EmptySlot),
            ),
            PickupRandomInput::None,
        )
        .unwrap();
    let mut wrong = PickupSource::new(PickupSourceId::new(2).unwrap(), stack(ITEM_A, 1, 1));
    assert!(matches!(
        equipment.commit_mob_pickup(&mut wrong, transaction),
        Err(PickupCommitError::WrongSource { .. })
    ));
    assert!(equipment.main_hand().is_empty());
    assert_eq!(wrong.stack(), &stack(ITEM_A, 1, 1));
}

#[test]
fn occupied_pickup_requires_replace_decision_and_drop_roll() {
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::MainHand, stack(ITEM_B, 1, 1));
    let source = PickupSource::new(PickupSourceId::new(1).unwrap(), stack(ITEM_A, 1, 1));
    assert_eq!(
        equipment.prepare_mob_pickup(
            &source,
            pickup_facts(
                &equipment,
                EquipmentSlot::MainHand,
                ReplacementDecision::Keep(ReplacementReason::WorseAttackDamage),
            ),
            PickupRandomInput::None,
        ),
        Err(PickupPrepareError::CannotReplace)
    );
    assert_eq!(
        equipment.prepare_mob_pickup(
            &source,
            pickup_facts(
                &equipment,
                EquipmentSlot::MainHand,
                replace(ReplacementReason::BetterAttackDamage),
            ),
            PickupRandomInput::None,
        ),
        Err(PickupPrepareError::MissingReplacementDropRoll)
    );
}

fn action_slots(batch: &EquipmentPublicationBatch) -> Vec<(u8, EquipmentSlot)> {
    batch
        .actions()
        .iter()
        .filter_map(|action| match action {
            EquipmentPublicationAction::RemoveLocationEffects { slot, .. } => Some((0, *slot)),
            EquipmentPublicationAction::ApplyLocationEffects { slot, .. } => Some((1, *slot)),
            _ => None,
        })
        .collect()
}

fn admit_batch(batch: EquipmentPublicationBatch) -> PublicationToken {
    batch.token()
}

#[test]
fn publication_batch_is_ordered_and_baseline_advances_only_after_admission() {
    let mut equipment = EquipmentState::new();
    let main = stack(ITEM_A, 1, 1);
    let off = stack(ITEM_B, 1, 2);
    equipment.set(EquipmentSlot::MainHand, main.clone());
    equipment.set(EquipmentSlot::OffHand, off.clone());
    equipment.initialize_publication_baseline().unwrap();
    equipment.set(EquipmentSlot::MainHand, off.clone());
    equipment.set(EquipmentSlot::OffHand, main.clone());
    let head = stack(ITEM_A, 1, 3);
    equipment.set(EquipmentSlot::Head, head.clone());

    let PublicationPrepareOutcome::Prepared(batch) =
        equipment.prepare_equipment_publication().unwrap()
    else {
        panic!("changes must produce a batch");
    };
    assert_eq!(
        action_slots(&batch),
        vec![
            (0, EquipmentSlot::MainHand),
            (0, EquipmentSlot::OffHand),
            (1, EquipmentSlot::MainHand),
            (1, EquipmentSlot::OffHand),
            (1, EquipmentSlot::Head),
        ]
    );
    assert!(matches!(
        batch.actions()[5],
        EquipmentPublicationAction::HandSwapEvent { event_id: 55 }
    ));
    assert!(matches!(
        &batch.actions()[6],
        EquipmentPublicationAction::EquipmentPacket(entries)
            if entries.len() == 1 && entries.get(0).unwrap().slot == EquipmentSlot::Head
    ));
    assert_eq!(
        equipment.published(EquipmentSlot::Head),
        &ItemStackState::EMPTY
    );

    let token = admit_batch(batch);
    equipment.confirm_publication_admitted(token).unwrap();
    assert_eq!(equipment.published(EquipmentSlot::Head), &head);
    assert_eq!(
        equipment.prepare_equipment_publication().unwrap(),
        PublicationPrepareOutcome::NoChanges
    );
}

#[test]
fn publication_rejection_and_stale_confirmation_never_advance_baseline() {
    let mut equipment = EquipmentState::new();
    let first = stack(ITEM_A, 1, 1);
    equipment.set(EquipmentSlot::Head, first.clone());
    let PublicationPrepareOutcome::Prepared(batch) =
        equipment.prepare_equipment_publication().unwrap()
    else {
        panic!("batch required");
    };
    assert!(matches!(
        equipment.confirm_publication_admitted(PublicationToken::new(999).unwrap()),
        Err(PublicationAdmissionError::StaleToken { .. })
    ));
    assert_eq!(
        equipment.published(EquipmentSlot::Head),
        &ItemStackState::EMPTY
    );
    assert!(matches!(
        equipment.prepare_equipment_publication(),
        Err(PublicationPrepareError::AwaitingAdmission { .. })
    ));
    equipment.abort_publication_admission(batch).unwrap();
    assert_eq!(
        equipment.published(EquipmentSlot::Head),
        &ItemStackState::EMPTY
    );
    assert!(matches!(
        equipment.prepare_equipment_publication().unwrap(),
        PublicationPrepareOutcome::Prepared(_)
    ));
}

#[test]
fn publication_confirmation_and_abort_require_a_pending_candidate() {
    let mut equipment = EquipmentState::new();
    let token = PublicationToken::new(1).unwrap();
    assert_eq!(
        equipment.confirm_publication_admitted(token),
        Err(PublicationAdmissionError::NoPendingAdmission)
    );

    equipment.set(EquipmentSlot::Head, stack(ITEM_A, 1, 1));
    let PublicationPrepareOutcome::Prepared(batch) =
        equipment.prepare_equipment_publication().unwrap()
    else {
        panic!("batch required");
    };
    assert!(matches!(
        equipment.initialize_publication_baseline(),
        Err(PublicationPrepareError::AwaitingAdmission { .. })
    ));
    equipment.abort_publication_admission(batch).unwrap();
    assert_eq!(
        equipment.confirm_publication_admitted(token),
        Err(PublicationAdmissionError::NoPendingAdmission)
    );
}

#[test]
fn mutation_while_batch_awaits_admission_is_published_next() {
    let mut equipment = EquipmentState::new();
    let first = stack(ITEM_A, 1, 1);
    let second = stack(ITEM_B, 1, 2);
    equipment.set(EquipmentSlot::Head, first.clone());
    let PublicationPrepareOutcome::Prepared(batch) =
        equipment.prepare_equipment_publication().unwrap()
    else {
        panic!("batch required");
    };
    equipment.set(EquipmentSlot::Head, second);
    let token = admit_batch(batch);
    equipment.confirm_publication_admitted(token).unwrap();
    assert_eq!(equipment.published(EquipmentSlot::Head), &first);
    assert!(matches!(
        equipment.prepare_equipment_publication().unwrap(),
        PublicationPrepareOutcome::Prepared(_)
    ));
}

#[test]
fn durability_nonfinite_cast_matches_java_result() {
    assert_eq!(durability_damage_from_hurt(f32::NAN), None);
    assert_eq!(durability_damage_from_hurt(f32::NEG_INFINITY), None);
    assert_eq!(durability_damage_from_hurt(0.0), None);
    assert_eq!(durability_damage_from_hurt(7.9), Some(1));
    assert_eq!(durability_damage_from_hurt(8.0), Some(2));
    assert_eq!(durability_damage_from_hurt(f32::INFINITY), Some(i32::MAX));
}

#[test]
fn durability_break_mutates_stack_and_returns_event_in_one_outcome() {
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::Body, damageable_stack(ITEM_A, 1, 9, 10));
    let revision = equipment.revision(EquipmentSlot::Body);
    let outcome = equipment
        .apply_equipment_durability(
            EquipmentSlot::Body,
            revision,
            ProcessedDurabilityChange::Apply(1),
        )
        .unwrap();
    assert!(matches!(
        outcome,
        DurabilityOutcome::Broken {
            event_id: 65,
            remaining,
            ..
        } if remaining.is_empty()
    ));
    assert!(equipment.get(EquipmentSlot::Body).is_empty());
}

fn death_facts() -> DeathDropFacts {
    DeathDropFacts {
        original_chance: 0.5,
        processed_chance: 0.5,
        killed_by_player: true,
        prevents_equipment_drop: false,
    }
}

#[test]
fn death_drop_clear_and_spawn_fact_are_one_owner_local_commit() {
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::Head, damageable_stack(ITEM_A, 1, 0, 10));
    let plan = equipment.plan_death_drop(EquipmentSlot::Head, death_facts());
    let outcome = equipment
        .commit_death_drop(
            plan,
            Some(DropRoll::new(0.0).unwrap()),
            Some(DeathDropDurabilityRandom {
                inner: NextIntSample::new(7, 6).unwrap(),
                outer: NextIntSample::new(7, 6).unwrap(),
            }),
        )
        .unwrap();
    let DeathDropOutcome::Dropped { spawn, .. } = outcome else {
        panic!("roll must drop");
    };
    assert_eq!(spawn.damage_value(), Some(4));
    assert!(equipment.get(EquipmentSlot::Head).is_empty());
}

#[test]
fn stale_death_drop_plan_neither_clears_nor_returns_spawn() {
    let mut equipment = EquipmentState::new();
    let original = damageable_stack(ITEM_A, 1, 0, 10);
    equipment.set(EquipmentSlot::Head, original);
    let plan = equipment.plan_death_drop(EquipmentSlot::Head, death_facts());
    let replacement = stack(ITEM_B, 1, 2);
    equipment.set(EquipmentSlot::Head, replacement.clone());
    assert!(matches!(
        equipment.commit_death_drop(
            plan,
            Some(DropRoll::new(0.0).unwrap()),
            Some(DeathDropDurabilityRandom {
                inner: NextIntSample::new(7, 0).unwrap(),
                outer: NextIntSample::new(1, 0).unwrap(),
            }),
        ),
        Err(DeathDropCommitError::StaleEquipment { .. })
    ));
    assert_eq!(equipment.get(EquipmentSlot::Head), &replacement);
}

#[test]
fn failed_death_roll_does_not_clear_slot_or_accept_unused_rng() {
    let mut equipment = EquipmentState::new();
    let original = damageable_stack(ITEM_A, 1, 0, 10);
    equipment.set(EquipmentSlot::Head, original.clone());
    let plan = equipment.plan_death_drop(EquipmentSlot::Head, death_facts());
    assert_eq!(
        equipment.commit_death_drop(plan, Some(DropRoll::new(0.9).unwrap()), None),
        Ok(DeathDropOutcome::NotDropped)
    );
    assert_eq!(equipment.get(EquipmentSlot::Head), &original);

    let plan = equipment.plan_death_drop(EquipmentSlot::Head, death_facts());
    assert_eq!(
        equipment.commit_death_drop(
            plan,
            Some(DropRoll::new(0.9).unwrap()),
            Some(DeathDropDurabilityRandom {
                inner: NextIntSample::new(7, 0).unwrap(),
                outer: NextIntSample::new(1, 0).unwrap(),
            }),
        ),
        Err(DeathDropCommitError::Random(
            DeathDropRandomError::UnexpectedDurabilityRandom
        ))
    );
    assert_eq!(equipment.get(EquipmentSlot::Head), &original);
}

#[test]
fn nested_next_int_bounds_are_exact_and_caller_generated() {
    assert_eq!(
        NextIntSample::new(0, 0),
        Err(NextIntError::NonPositiveBound)
    );
    assert_eq!(
        NextIntSample::new(2, 2),
        Err(NextIntError::ValueOutsideBound)
    );
    let mut equipment = EquipmentState::new();
    equipment.set(EquipmentSlot::Head, damageable_stack(ITEM_A, 1, 0, 10));
    let plan = equipment.plan_death_drop(EquipmentSlot::Head, death_facts());
    assert!(matches!(
        equipment.commit_death_drop(
            plan,
            Some(DropRoll::new(0.0).unwrap()),
            Some(DeathDropDurabilityRandom {
                inner: NextIntSample::new(6, 0).unwrap(),
                outer: NextIntSample::new(1, 0).unwrap(),
            }),
        ),
        Err(DeathDropCommitError::Random(
            DeathDropRandomError::InnerBound {
                expected: 7,
                actual: 6
            }
        ))
    ));
    assert!(!equipment.get(EquipmentSlot::Head).is_empty());
}

#[test]
fn death_drop_missing_and_outer_rng_failures_leave_equipment_owned() {
    let mut equipment = EquipmentState::new();
    let original = damageable_stack(ITEM_A, 1, 0, 10);
    equipment.set(EquipmentSlot::Head, original.clone());
    let plan = equipment.plan_death_drop(EquipmentSlot::Head, death_facts());
    assert_eq!(
        equipment.commit_death_drop(plan, None, None),
        Err(DeathDropCommitError::Random(
            DeathDropRandomError::MissingDropRoll
        ))
    );

    let plan = equipment.plan_death_drop(EquipmentSlot::Head, death_facts());
    assert_eq!(
        equipment.commit_death_drop(plan, Some(DropRoll::new(0.0).unwrap()), None),
        Err(DeathDropCommitError::Random(
            DeathDropRandomError::MissingDurabilityRandom
        ))
    );

    let plan = equipment.plan_death_drop(EquipmentSlot::Head, death_facts());
    assert!(matches!(
        equipment.commit_death_drop(
            plan,
            Some(DropRoll::new(0.0).unwrap()),
            Some(DeathDropDurabilityRandom {
                inner: NextIntSample::new(7, 3).unwrap(),
                outer: NextIntSample::new(3, 0).unwrap(),
            }),
        ),
        Err(DeathDropCommitError::Random(
            DeathDropRandomError::OuterBound {
                expected: 4,
                actual: 3
            }
        ))
    ));
    assert_eq!(equipment.get(EquipmentSlot::Head), &original);
}

#[test]
fn skipped_death_drop_rejects_unused_roll_without_mutation() {
    let mut equipment = EquipmentState::new();
    let original = stack(ITEM_A, 1, 1);
    equipment.set(EquipmentSlot::Head, original.clone());
    let plan = equipment.plan_death_drop(
        EquipmentSlot::Head,
        DeathDropFacts {
            original_chance: 0.0,
            ..death_facts()
        },
    );
    assert_eq!(
        equipment.commit_death_drop(plan, Some(DropRoll::new(0.0).unwrap()), None),
        Err(DeathDropCommitError::Random(
            DeathDropRandomError::UnexpectedDropRoll
        ))
    );
    assert_eq!(equipment.get(EquipmentSlot::Head), &original);
}

#[test]
fn hand_swap_revision_preconditions_are_atomic() {
    let mut equipment = EquipmentState::new();
    let main = stack(ITEM_A, 1, 1);
    let off = stack(ITEM_B, 1, 2);
    equipment.set(EquipmentSlot::MainHand, main.clone());
    equipment.set(EquipmentSlot::OffHand, off.clone());
    let main_revision = equipment.revision(EquipmentSlot::MainHand);
    let off_revision = equipment.revision(EquipmentSlot::OffHand);
    assert!(matches!(
        equipment.swap_hands(HandSwapPrecondition {
            main_hand: SlotRevision::INITIAL,
            off_hand: off_revision,
        }),
        Err(EquipmentMutationError::StaleRevision { .. })
    ));
    assert_eq!(equipment.hand_items(), [&main, &off]);
    equipment
        .swap_hands(HandSwapPrecondition {
            main_hand: main_revision,
            off_hand: off_revision,
        })
        .unwrap();
    assert_eq!(equipment.hand_items(), [&off, &main]);
}

#[test]
fn body_and_saddle_replacement_rules_remain_distinct() {
    let equal = EqualItemFacts {
        new_enchantment_entries: 0,
        current_enchantment_entries: 0,
        new_damage: 0,
        current_damage: 0,
        new_has_custom_name: false,
        current_has_custom_name: false,
    };
    assert!(
        decide_replacement(
            EquipmentSlot::Body,
            CurrentItemFacts::Occupied(ReplacementComparison::Armor(ArmorReplacementFacts {
                current_prevents_armor_change: false,
                new_defense: 5.0,
                current_defense: 4.0,
                new_toughness: 0.0,
                current_toughness: 0.0,
                equal_item: equal,
            }))
        )
        .unwrap()
        .should_replace()
    );
    assert!(
        !decide_replacement(
            EquipmentSlot::Saddle,
            CurrentItemFacts::Occupied(ReplacementComparison::NotApplicable)
        )
        .unwrap()
        .should_replace()
    );
}
