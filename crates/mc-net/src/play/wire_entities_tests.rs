use bytes::BytesMut;
use mc_data::Identifier;
use mc_entity::effects_26_1_2::{
    EffectDamageSource, EffectFlags, EffectId, EffectInstance, EffectKind, TargetEffectContext,
};
use mc_entity::living_26_1_2::DamageContext;
use mc_entity::runtime_26_1_2::{EffectAction, PublicationFact, TargetKind};
use mc_entity::{
    AnimalBreedingState, AttributeKind, EntityDamageRequest, EntityEffectOperation,
    EntityEffectRejection, EntityEffectRequest, EntityEffectResult, EntityId, EntityItemStack,
    EntitySnapshot, EntityStore, Rotation, SheepColor, SpawnEntity, Vec3, VillagerData,
    VillagerKind, VillagerProfession,
};
use mc_protocol::Packet;
use mc_protocol::frame::{Compression, try_decode_frame};
use mc_protocol::packets::play::{
    ClientboundSetEntityData, EntityDataValue, EntityPositionSync, EntityVec3,
    ITEM_ENTITY_DATA_ITEM_INDEX, ItemStack, LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2, MoveEntityPos,
    MoveEntityPosRot, PositionMoveRotation, RotateHead, SetEntityMotion,
};
use std::collections::HashSet;

use super::persistence::PersistedEntityCheckpoint;
use super::session::{
    EntityAttackOutcome, EntityKillRewards, OutboundCommand, ServerEntityMove,
    ServerEntitySnapshot, SessionRegistry, dispatch_visibility_commands,
    server_entity_snapshot_from,
};
use super::simulation::{SimulationAuthority, simulation_channel};
use super::wire_entities::{
    MoveEntityRot, ServerEntityWireMove, send_entity_health, send_entity_pairing_data,
    send_entity_relative_move,
};

fn entity_snapshot(type_id: i32, type_name: &str) -> ServerEntitySnapshot {
    ServerEntitySnapshot {
        id: EntityId(42),
        uuid: uuid::Uuid::from_u128(42),
        type_id,
        type_name: type_name.to_owned(),
        position: Vec3::new(1.5, 64.0, 1.5),
        rotation: Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        health: None,
        item_stack: None,
        experience_value: None,
        block_state: None,
        animal: None,
        villager: None,
    }
}

#[tokio::test]
async fn item_entity_pairing_preserves_loader_presentation_components() {
    let model = Identifier::parse("solaris_loader:loader_block").unwrap();
    let mut snapshot = entity_snapshot(71, "minecraft:item");
    snapshot.item_stack = Some(
        EntityItemStack::new(45, 1)
            .with_custom_name("Ruby Block")
            .with_item_model(model.clone()),
    );
    let mut writer = Vec::new();

    send_entity_pairing_data(&mut writer, Compression::Disabled, &snapshot)
        .await
        .unwrap();

    let mut bytes = BytesMut::from(writer.as_slice());
    let mut frame = try_decode_frame(&mut bytes, Compression::Disabled)
        .unwrap()
        .expect("set entity data frame");
    assert_eq!(frame.id, ClientboundSetEntityData::ID);
    assert_eq!(
        ClientboundSetEntityData::decode(&mut frame.body).unwrap(),
        ClientboundSetEntityData {
            entity_id: 42,
            values: vec![EntityDataValue::ItemStack {
                index: ITEM_ENTITY_DATA_ITEM_INDEX,
                stack: ItemStack::new(45, 1)
                    .with_custom_name("Ruby Block")
                    .with_item_model(model),
            }],
        }
    );
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn sheep_snapshot_metadata_matches_26_1_2_pairing_non_defaults() {
    let mut snapshot = entity_snapshot(111, "minecraft:sheep");
    snapshot.health = Some(6.5);
    snapshot.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::Brown));
    let mut writer = Vec::new();

    send_entity_pairing_data(&mut writer, Compression::Disabled, &snapshot)
        .await
        .unwrap();

    let mut bytes = BytesMut::from(writer.as_slice());
    let mut frame = try_decode_frame(&mut bytes, Compression::Disabled)
        .unwrap()
        .expect("set entity data frame");
    assert_eq!(frame.id, ClientboundSetEntityData::ID);
    assert_eq!(
        frame.body.as_ref(),
        &[0x2a, 9, 3, 0x40, 0xd0, 0, 0, 18, 0, 12, 0xff]
    );
    assert_eq!(
        ClientboundSetEntityData::decode(&mut frame.body).unwrap(),
        ClientboundSetEntityData {
            entity_id: 42,
            values: vec![
                EntityDataValue::Float {
                    index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
                    value: 6.5,
                },
                EntityDataValue::Byte {
                    index: 18,
                    value: 12,
                },
            ],
        }
    );
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn damaged_cow_and_chicken_pairing_uses_snapshot_health() {
    let cases = [
        (
            30,
            "minecraft:cow",
            6.0,
            vec![0x2a, 9, 3, 0x40, 0xc0, 0, 0, 0xff],
        ),
        (
            26,
            "minecraft:chicken",
            2.5,
            vec![0x2a, 9, 3, 0x40, 0x20, 0, 0, 0xff],
        ),
    ];

    for (type_id, type_name, health, expected_body) in cases {
        let mut snapshot = entity_snapshot(type_id, type_name);
        snapshot.health = Some(health);
        snapshot.animal = Some(AnimalBreedingState::adult());
        let mut writer = Vec::new();

        send_entity_pairing_data(&mut writer, Compression::Disabled, &snapshot)
            .await
            .unwrap();

        let mut bytes = BytesMut::from(writer.as_slice());
        let mut frame = try_decode_frame(&mut bytes, Compression::Disabled)
            .unwrap()
            .expect("set entity data frame");
        assert_eq!(frame.id, ClientboundSetEntityData::ID);
        assert_eq!(frame.body.as_ref(), expected_body.as_slice());
        assert_eq!(
            ClientboundSetEntityData::decode(&mut frame.body).unwrap(),
            ClientboundSetEntityData {
                entity_id: 42,
                values: vec![EntityDataValue::Float {
                    index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
                    value: health,
                }],
            }
        );
        assert!(bytes.is_empty());
    }
}

#[tokio::test]
async fn unknown_type_does_not_project_animal_metadata() {
    let mut snapshot = entity_snapshot(999, "solaris:unknown");
    snapshot.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::Brown));
    let mut writer = Vec::new();

    send_entity_pairing_data(&mut writer, Compression::Disabled, &snapshot)
        .await
        .unwrap();

    assert!(writer.is_empty());
}

#[tokio::test]
async fn cow_snapshot_does_not_project_sheep_wool() {
    let mut snapshot = entity_snapshot(30, "minecraft:cow");
    snapshot.health = Some(7.0);
    snapshot.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::Brown));
    let mut writer = Vec::new();

    send_entity_pairing_data(&mut writer, Compression::Disabled, &snapshot)
        .await
        .unwrap();

    let mut bytes = BytesMut::from(writer.as_slice());
    let mut frame = try_decode_frame(&mut bytes, Compression::Disabled)
        .unwrap()
        .expect("set entity data frame");
    assert_eq!(frame.id, ClientboundSetEntityData::ID);
    assert_eq!(frame.body.as_ref(), &[0x2a, 9, 3, 0x40, 0xe0, 0, 0, 0xff]);
    assert_eq!(
        ClientboundSetEntityData::decode(&mut frame.body)
            .unwrap()
            .values,
        vec![EntityDataValue::Float {
            index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
            value: 7.0,
        }]
    );
    assert!(bytes.is_empty());
}

fn decode_entity_data(writer: &[u8]) -> Vec<ClientboundSetEntityData> {
    let mut bytes = BytesMut::from(writer);
    let mut packets = Vec::new();
    while !bytes.is_empty() {
        let mut frame = try_decode_frame(&mut bytes, Compression::Disabled)
            .unwrap()
            .expect("complete entity data frame");
        assert_eq!(frame.id, ClientboundSetEntityData::ID);
        packets.push(ClientboundSetEntityData::decode(&mut frame.body).unwrap());
    }
    packets
}

#[tokio::test]
async fn pairing_omits_vanilla_default_health_but_incremental_does_not() {
    let mut snapshot = entity_snapshot(30, "minecraft:cow");
    snapshot.health = Some(1.0);
    let mut pairing = Vec::new();
    let mut incremental = Vec::new();

    send_entity_pairing_data(&mut pairing, Compression::Disabled, &snapshot)
        .await
        .unwrap();
    send_entity_health(&mut incremental, Compression::Disabled, &snapshot)
        .await
        .unwrap();

    assert!(pairing.is_empty());
    assert_eq!(
        decode_entity_data(&incremental)[0].values,
        vec![EntityDataValue::Float {
            index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
            value: 1.0,
        }]
    );
}

#[tokio::test]
async fn pairing_emits_plains_toolsmith_villager_data() {
    let mut snapshot = entity_snapshot(120, "minecraft:villager");
    snapshot.villager = Some(VillagerData::new(
        VillagerKind::Plains,
        VillagerProfession::Toolsmith,
        1,
    ));
    let mut writer = Vec::new();

    send_entity_pairing_data(&mut writer, Compression::Disabled, &snapshot)
        .await
        .unwrap();

    assert_eq!(
        decode_entity_data(&writer)[0].values,
        vec![EntityDataValue::VillagerData {
            index: 19,
            villager_type: 2,
            profession: 13,
            level: 1,
        }]
    );
}

#[tokio::test]
async fn non_living_snapshot_does_not_emit_health_metadata() {
    let snapshot = entity_snapshot(2, "minecraft:arrow");
    let mut writer = Vec::new();

    send_entity_health(&mut writer, Compression::Disabled, &snapshot)
        .await
        .unwrap();

    assert!(writer.is_empty());
}

#[tokio::test]
async fn pairing_emits_health_once_without_default_overwrite() {
    let mut snapshot = entity_snapshot(111, "minecraft:sheep");
    snapshot.health = Some(3.25);
    snapshot.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::Brown));
    let mut writer = Vec::new();

    send_entity_pairing_data(&mut writer, Compression::Disabled, &snapshot)
        .await
        .unwrap();

    let packets = decode_entity_data(&writer);
    let health_values = packets[0]
        .values
        .iter()
        .filter(|value| {
            matches!(
                value,
                EntityDataValue::Float {
                    index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
                    ..
                }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        health_values,
        vec![&EntityDataValue::Float {
            index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
            value: 3.25,
        }]
    );
}

fn observed_entity(
    type_id: i32,
    type_name: &str,
) -> (
    SessionRegistry,
    EntityId,
    tokio::sync::mpsc::Receiver<OutboundCommand>,
) {
    let registry = SessionRegistry::new();
    let profile = crate::login::LoggedInProfile {
        uuid: crate::login::offline_uuid("HealthObserver"),
        name: "HealthObserver".to_owned(),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        super::PlayerPose::new(0.5, 64.0, 0.5),
    );
    registry.mark_loaded(session_id, (0, 0));
    let spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        type_id,
        type_name.to_owned(),
        Vec3::new(1.5, 64.0, 1.5),
    );
    let entity_id = spawn
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(snapshot) => Some(snapshot.id),
            _ => None,
        })
        .expect("loaded observer receives cow spawn");
    dispatch_visibility_commands(spawn);
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::SpawnEntity(snapshot)) if snapshot.id == entity_id
    ));
    assert!(rx.try_recv().is_err(), "fixture publishes one spawn");
    (registry, entity_id, rx)
}

fn observed_cow() -> (
    SessionRegistry,
    EntityId,
    tokio::sync::mpsc::Receiver<OutboundCommand>,
) {
    observed_entity(30, "minecraft:cow")
}

fn health_updates(outcome: &EntityAttackOutcome) -> Vec<f32> {
    outcome
        .dispatches()
        .iter()
        .filter_map(|dispatch| match &dispatch.command {
            OutboundCommand::UpdateEntityHealth(snapshot) => snapshot.health,
            _ => None,
        })
        .collect()
}

fn current_entity_snapshot(
    registry: &SessionRegistry,
    entity_id: EntityId,
) -> mc_entity::EntitySnapshot {
    registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == entity_id)
        .expect("authoritative entity snapshot")
        .snapshot
}

async fn drive_entity_effect(
    simulation: &super::simulation::SimulationHandle,
    owner: &mut super::simulation::SimulationOwner,
    registry: &SessionRegistry,
    entity_id: EntityId,
    expected: Option<EntitySnapshot>,
    operation: EntityEffectOperation,
) -> EntityEffectResult {
    let mut request = Box::pin(simulation.apply_entity_effect(
        entity_id,
        expected,
        operation,
        TargetKind::NonPlayer,
    ));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(request.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(owner.process_tick(registry, 1).processed, 1);
    request.await.expect("effect owner response")
}

#[tokio::test(flavor = "current_thread")]
async fn accepted_effect_heal_mutates_owner_and_publishes_clamped_health() {
    let (registry, entity_id, mut outbound) = observed_cow();
    let damaged = registry
        .attack_server_entity(
            &SimulationAuthority::for_test(),
            entity_id,
            4.0,
            None,
            &EntityKillRewards::default(),
        )
        .expect("damage accepted");
    assert_eq!(health_updates(&damaged), vec![6.0]);
    dispatch_visibility_commands(damaged.into_dispatches());
    assert!(matches!(
        outbound.try_recv(),
        Ok(OutboundCommand::UpdateEntityHealth(snapshot)) if snapshot.health == Some(6.0)
    ));
    assert!(outbound.try_recv().is_err());
    let expected = current_entity_snapshot(&registry, entity_id);
    let max_health = expected
        .attributes
        .base(&AttributeKind::MaxHealth)
        .expect("cow max health");

    let (simulation, mut owner) = simulation_channel();
    let mut request = Box::pin(simulation.apply_entity_effect(
        entity_id,
        Some(expected),
        EntityEffectOperation::ApplyAction {
            effect_id: EffectId::new(0),
            action: EffectAction::Heal { amount: 100.0 },
            damage_context: None,
        },
        TargetKind::NonPlayer,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(request.as_mut(), cx).is_pending(),
            "effect heal waits for its owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let result = request.await.expect("effect heal owner response");

    let EntityEffectResult::Applied(applied) = &result else {
        panic!("over-max effect heal must be accepted and clamped");
    };
    assert_eq!(applied.snapshot.health, max_health as f32);
    assert_eq!(
        applied.snapshot.attributes.base(&AttributeKind::MaxHealth),
        Some(max_health)
    );
    assert_eq!(
        current_entity_snapshot(&registry, entity_id),
        applied.snapshot
    );
    let published = outbound
        .try_recv()
        .expect("accepted owner heal publishes one health update");
    let OutboundCommand::UpdateEntityHealth(snapshot) = published else {
        panic!("accepted owner heal published a different command: {published:?}");
    };
    assert_eq!(snapshot.health, Some(max_health as f32));
    assert!(outbound.try_recv().is_err(), "heal publishes exactly once");

    let mut writer = Vec::new();
    send_entity_health(&mut writer, Compression::Disabled, &snapshot)
        .await
        .unwrap();
    assert_eq!(
        decode_entity_data(&writer),
        vec![ClientboundSetEntityData {
            entity_id: entity_id.0,
            values: vec![EntityDataValue::Float {
                index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
                value: max_health as f32,
            }],
        }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn public_effect_ingress_commits_damage_heal_and_expiry_once() {
    let (registry, entity_id, mut outbound) = observed_cow();
    let (simulation, mut owner) = simulation_channel();

    let damaged = drive_entity_effect(
        &simulation,
        &mut owner,
        &registry,
        entity_id,
        None,
        EntityEffectOperation::ApplyAction {
            effect_id: EffectId::new(7),
            action: EffectAction::Damage {
                amount: 2.0,
                source: EffectDamageSource::Magic,
            },
            damage_context: Some(DamageContext::default()),
        },
    )
    .await;
    let EntityEffectResult::Applied(damaged) = damaged else {
        panic!("effect damage must commit");
    };
    assert_eq!(damaged.snapshot.health, 8.0);
    assert!(matches!(
        damaged.publications.first(),
        Some(PublicationFact::DamageApplied { .. })
    ));

    let instant_health = EffectInstance::new(
        EffectId::new(6),
        EffectKind::InstantHealth,
        1,
        0,
        EffectFlags::default(),
    );
    assert!(matches!(
        drive_entity_effect(
            &simulation,
            &mut owner,
            &registry,
            entity_id,
            Some(damaged.snapshot.clone()),
            EntityEffectOperation::Add(instant_health),
        )
        .await,
        EntityEffectResult::Applied(_)
    ));
    let expected_tick = current_entity_snapshot(&registry, entity_id);
    let healed = drive_entity_effect(
        &simulation,
        &mut owner,
        &registry,
        entity_id,
        Some(expected_tick),
        EntityEffectOperation::Tick {
            entity_tick_count: 2,
            target_context: TargetEffectContext::LIVING,
            damage_context: DamageContext::default(),
        },
    )
    .await;
    let EntityEffectResult::Applied(healed) = healed else {
        panic!("instant-health tick must commit");
    };
    assert_eq!(healed.snapshot.health, 10.0);
    assert!(matches!(
        healed.publications.as_slice(),
        [PublicationFact::HealthChanged { before: 8.0, after: 10.0, .. },
         PublicationFact::EffectRemoved { effect }]
            if effect.id == instant_health.id && effect.duration == 0
    ));
    assert_eq!(
        drive_entity_effect(
            &simulation,
            &mut owner,
            &registry,
            entity_id,
            Some(healed.snapshot),
            EntityEffectOperation::Tick {
                entity_tick_count: 3,
                target_context: TargetEffectContext::LIVING,
                damage_context: DamageContext::default(),
            },
        )
        .await,
        EntityEffectResult::Rejected(EntityEffectRejection::NoActiveEffects)
    );

    let updates = std::iter::from_fn(|| outbound.try_recv().ok())
        .filter_map(|command| match command {
            OutboundCommand::UpdateEntityHealth(snapshot) => snapshot.health,
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(updates, vec![8.0, 10.0]);
}

#[test]
fn effect_ingress_preserves_capacity_rejection_without_publication() {
    let (registry, entity_id, mut outbound) = observed_cow();

    for raw_id in 0..32 {
        let expected = current_entity_snapshot(&registry, entity_id);
        let (result, dispatches) = registry.apply_server_entity_effect_request(
            &SimulationAuthority::for_test(),
            Some(expected),
            entity_id,
            EntityEffectRequest {
                operation: EntityEffectOperation::Add(EffectInstance::new(
                    EffectId::new(raw_id),
                    EffectKind::CallerOwned,
                    100,
                    0,
                    EffectFlags::default(),
                )),
                target_kind: TargetKind::NonPlayer,
                death_remove_tick: 20,
            },
        );
        assert!(matches!(result, EntityEffectResult::Applied(_)));
        assert!(dispatches.is_empty());
    }

    let before = current_entity_snapshot(&registry, entity_id);
    let (result, dispatches) = registry.apply_server_entity_effect_request(
        &SimulationAuthority::for_test(),
        Some(before.clone()),
        entity_id,
        EntityEffectRequest {
            operation: EntityEffectOperation::Add(EffectInstance::new(
                EffectId::new(32),
                EffectKind::CallerOwned,
                100,
                0,
                EffectFlags::default(),
            )),
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 20,
        },
    );

    assert_eq!(
        result,
        EntityEffectResult::Rejected(EntityEffectRejection::EffectCapacity)
    );
    assert!(dispatches.is_empty());
    assert_eq!(current_entity_snapshot(&registry, entity_id), before);
    assert!(outbound.try_recv().is_err());
}

#[test]
fn rejected_effect_actions_preserve_exact_outcomes_without_mutation_or_publication() {
    let cases = [
        (
            EffectAction::Heal { amount: 0.0 },
            EntityEffectRejection::InvalidAction,
        ),
        (
            EffectAction::Heal { amount: -1.0 },
            EntityEffectRejection::InvalidAction,
        ),
        (
            EffectAction::Heal { amount: f32::NAN },
            EntityEffectRejection::InvalidAction,
        ),
        (
            EffectAction::HealIfBelowMax { amount: 1.0 },
            EntityEffectRejection::AtMaxHealth,
        ),
        (
            EffectAction::Damage {
                amount: 1.0,
                source: mc_entity::effects_26_1_2::EffectDamageSource::Magic,
            },
            EntityEffectRejection::UnresolvedDamageContext,
        ),
    ];

    for (action, expected_rejection) in cases {
        let (registry, entity_id, mut outbound) = observed_cow();
        let before = current_entity_snapshot(&registry, entity_id);
        let (result, dispatches) = registry.apply_server_entity_effect_request(
            &SimulationAuthority::for_test(),
            Some(before.clone()),
            entity_id,
            EntityEffectRequest {
                operation: EntityEffectOperation::ApplyAction {
                    effect_id: EffectId::new(0),
                    action,
                    damage_context: None,
                },
                target_kind: TargetKind::NonPlayer,
                death_remove_tick: 20,
            },
        );

        assert_eq!(result, EntityEffectResult::Rejected(expected_rejection));
        assert!(dispatches.is_empty());
        assert_eq!(current_entity_snapshot(&registry, entity_id), before);
        assert_eq!(
            registry.published_entity_health_for_test(entity_id),
            Some(before.health)
        );
        assert!(outbound.try_recv().is_err(), "rejected heal cannot publish");
    }
}

#[test]
fn stale_and_dead_effect_heals_do_not_publish() {
    let (stale_registry, stale_id, mut stale_outbound) = observed_cow();
    let stale = current_entity_snapshot(&stale_registry, stale_id);
    let damage = stale_registry
        .attack_server_entity(
            &SimulationAuthority::for_test(),
            stale_id,
            3.0,
            None,
            &EntityKillRewards::default(),
        )
        .expect("damage accepted");
    assert_eq!(health_updates(&damage), vec![7.0]);
    let current = current_entity_snapshot(&stale_registry, stale_id);
    let (rejected, dispatches) = stale_registry.apply_server_entity_effect_request(
        &SimulationAuthority::for_test(),
        Some(stale),
        stale_id,
        EntityEffectRequest {
            operation: EntityEffectOperation::ApplyAction {
                effect_id: EffectId::new(0),
                action: EffectAction::Heal { amount: 1.0 },
                damage_context: None,
            },
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 20,
        },
    );
    assert_eq!(
        rejected,
        EntityEffectResult::Rejected(EntityEffectRejection::Stale)
    );
    assert!(dispatches.is_empty());
    assert_eq!(current_entity_snapshot(&stale_registry, stale_id), current);
    assert_eq!(
        stale_registry.published_entity_health_for_test(stale_id),
        Some(current.health)
    );
    assert!(
        stale_outbound.try_recv().is_err(),
        "stale heal cannot publish"
    );

    let (dead_registry, dead_id, mut dead_outbound) = observed_cow();
    dead_registry
        .attack_server_entity(
            &SimulationAuthority::for_test(),
            dead_id,
            20.0,
            None,
            &EntityKillRewards::default(),
        )
        .expect("lethal damage accepted");
    let dead = current_entity_snapshot(&dead_registry, dead_id);
    let (rejected, dispatches) = dead_registry.apply_server_entity_effect_request(
        &SimulationAuthority::for_test(),
        Some(dead.clone()),
        dead_id,
        EntityEffectRequest {
            operation: EntityEffectOperation::ApplyAction {
                effect_id: EffectId::new(0),
                action: EffectAction::Heal { amount: 1.0 },
                damage_context: None,
            },
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 20,
        },
    );
    assert_eq!(
        rejected,
        EntityEffectResult::Rejected(EntityEffectRejection::Dead)
    );
    assert!(dispatches.is_empty());
    assert_eq!(current_entity_snapshot(&dead_registry, dead_id), dead);
    assert_eq!(
        dead_registry.published_entity_health_for_test(dead_id),
        Some(0.0)
    );
    assert!(
        dead_outbound.try_recv().is_err(),
        "dead heal cannot publish"
    );
}

#[test]
fn non_living_effect_heal_is_rejected_without_publication() {
    let (registry, entity_id, mut outbound) = observed_entity(2, "minecraft:arrow");
    let before = current_entity_snapshot(&registry, entity_id);

    let (result, dispatches) = registry.apply_server_entity_effect_request(
        &SimulationAuthority::for_test(),
        Some(before.clone()),
        entity_id,
        EntityEffectRequest {
            operation: EntityEffectOperation::ApplyAction {
                effect_id: EffectId::new(0),
                action: EffectAction::Heal { amount: 1.0 },
                damage_context: None,
            },
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 20,
        },
    );

    assert_eq!(
        result,
        EntityEffectResult::Rejected(EntityEffectRejection::NonLiving)
    );
    assert!(dispatches.is_empty());
    assert_eq!(current_entity_snapshot(&registry, entity_id), before);
    assert_eq!(registry.published_entity_health_for_test(entity_id), None);
    assert!(
        outbound.try_recv().is_err(),
        "non-living heal cannot publish"
    );
}

#[test]
fn missing_effect_heal_target_is_rejected_without_publication() {
    let (registry, entity_id, mut outbound) = observed_cow();
    let expected = current_entity_snapshot(&registry, entity_id);
    registry
        .attack_server_entity(
            &SimulationAuthority::for_test(),
            entity_id,
            20.0,
            None,
            &EntityKillRewards::default(),
        )
        .expect("lethal damage accepted");
    registry.tick_dying_entities(&SimulationAuthority::for_test(), 20);
    assert!(
        registry
            .persisted_entity_records()
            .into_iter()
            .all(|record| record.snapshot.id != entity_id)
    );

    let (result, dispatches) = registry.apply_server_entity_effect_request(
        &SimulationAuthority::for_test(),
        Some(expected),
        entity_id,
        EntityEffectRequest {
            operation: EntityEffectOperation::ApplyAction {
                effect_id: EffectId::new(0),
                action: EffectAction::Heal { amount: 1.0 },
                damage_context: None,
            },
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 20,
        },
    );

    assert_eq!(
        result,
        EntityEffectResult::Rejected(EntityEffectRejection::Missing)
    );
    assert!(dispatches.is_empty());
    assert!(
        outbound.try_recv().is_err(),
        "missing-target heal cannot publish"
    );
}

#[test]
fn invalid_max_health_rejects_heal_without_mutation_or_publication() {
    let mut entities = EntityStore::new();
    let id = entities.spawn(SpawnEntity::new(
        30,
        "minecraft:cow",
        Vec3::new(1.5, 64.0, 1.5),
    ));
    let mut invalid = entities.snapshot(id).expect("cow snapshot");
    invalid.attributes.set_base(AttributeKind::MaxHealth, 0.0);
    let registry = SessionRegistry::new();
    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(0, [invalid.clone()],)),
        1
    );

    let (result, dispatches) = registry.apply_server_entity_effect_request(
        &SimulationAuthority::for_test(),
        Some(invalid.clone()),
        id,
        EntityEffectRequest {
            operation: EntityEffectOperation::ApplyAction {
                effect_id: EffectId::new(0),
                action: EffectAction::Heal { amount: 1.0 },
                damage_context: None,
            },
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 20,
        },
    );

    assert_eq!(
        result,
        EntityEffectResult::Rejected(EntityEffectRejection::InvalidMaxHealth)
    );
    assert!(dispatches.is_empty());
    assert_eq!(current_entity_snapshot(&registry, id), invalid);
    assert_eq!(
        registry.published_entity_health_for_test(id),
        Some(invalid.health)
    );
}

#[test]
fn accepted_damage_and_death_publish_current_ecs_health() {
    let (damaged_registry, damaged_id, _damaged_outbound) = observed_cow();
    let damaged = damaged_registry
        .attack_server_entity(
            &SimulationAuthority::for_test(),
            damaged_id,
            3.0,
            None,
            &EntityKillRewards::default(),
        )
        .expect("damage accepted");
    assert_eq!(health_updates(&damaged), vec![7.0]);

    let (killed_registry, killed_id, _killed_outbound) = observed_cow();
    let killed = killed_registry
        .attack_server_entity(
            &SimulationAuthority::for_test(),
            killed_id,
            20.0,
            None,
            &EntityKillRewards::default(),
        )
        .expect("lethal damage accepted");
    assert!(matches!(&killed, EntityAttackOutcome::Killed { .. }));
    assert_eq!(health_updates(&killed), vec![0.0]);
    assert!(matches!(
        killed.dispatches().first().map(|dispatch| &dispatch.command),
        Some(OutboundCommand::UpdateEntityHealth(snapshot)) if snapshot.health == Some(0.0)
    ));
}

#[test]
fn stale_health_snapshot_cannot_overwrite_newer_accepted_publication() {
    let (registry, entity_id, _outbound) = observed_cow();
    let stale = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == entity_id)
        .expect("initial cow snapshot")
        .snapshot;
    let outcome = registry
        .attack_server_entity(
            &SimulationAuthority::for_test(),
            entity_id,
            3.0,
            None,
            &EntityKillRewards::default(),
        )
        .expect("damage accepted");
    assert_eq!(health_updates(&outcome), vec![7.0]);
    let accepted = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == entity_id)
        .expect("damaged cow snapshot")
        .snapshot;

    let duplicate = registry.publish_entity_health_snapshot_for_test(accepted);
    let rejected_stale = registry.publish_entity_health_snapshot_for_test(stale);

    assert!(duplicate.is_empty());
    assert!(rejected_stale.is_empty());
    assert_eq!(
        registry.published_entity_health_for_test(entity_id),
        Some(7.0)
    );
}

#[test]
fn server_snapshot_health_comes_from_accepted_ecs_damage_snapshot() {
    let mut entities = EntityStore::new();
    let mut cow = SpawnEntity::new(30, "minecraft:cow", Vec3::new(1.5, 64.0, 1.5));
    cow.attributes.set_base(AttributeKind::MaxHealth, 10.0);
    let id = entities.spawn(cow);
    let accepted = entities
        .damage(
            id,
            EntityDamageRequest {
                amount: 3.5,
                tick: 1,
                death_remove_tick: 21,
            },
        )
        .expect("damage is accepted");

    let projected = server_entity_snapshot_from(accepted.snapshot);

    assert_eq!(projected.health, Some(6.5));
}

#[test]
fn non_living_ecs_snapshot_has_no_health_projection() {
    let mut entities = EntityStore::new();
    let id = entities.spawn(SpawnEntity::new(
        2,
        "minecraft:arrow",
        Vec3::new(1.5, 64.0, 1.5),
    ));

    let projected = server_entity_snapshot_from(entities.snapshot(id).unwrap());

    assert_eq!(projected.health, None);
}

fn movement(wire_move: Option<ServerEntityWireMove>) -> ServerEntityMove {
    ServerEntityMove {
        id: EntityId(17),
        position: Vec3::new(4.0, 65.0, -3.0),
        wire_move,
        velocity: Vec3::new(1.0, 0.0, -2.0),
        rotation: Rotation {
            yaw: 90.0,
            pitch: -15.0,
            head_yaw: 80.0,
        },
        on_ground: false,
        send_velocity: false,
        send_head_rotation: false,
    }
}

fn decode_packet_ids(writer: &[u8]) -> Vec<i32> {
    let mut bytes = BytesMut::from(writer);
    let mut ids = Vec::new();
    while !bytes.is_empty() {
        let frame = try_decode_frame(&mut bytes, Compression::Disabled)
            .unwrap()
            .expect("complete entity movement frame");
        ids.push(frame.id);
    }
    ids
}

#[tokio::test]
async fn each_entity_movement_shape_selects_its_exact_packet() {
    let cases = [
        (
            Some(ServerEntityWireMove::Position {
                delta: Vec3::new(0.25, 0.0, -0.5),
            }),
            false,
            MoveEntityPos::ID,
        ),
        (
            Some(ServerEntityWireMove::Rotation),
            false,
            MoveEntityRot::ID,
        ),
        (
            Some(ServerEntityWireMove::PositionRotation {
                delta: Vec3::new(0.25, 0.0, -0.5),
            }),
            false,
            MoveEntityPosRot::ID,
        ),
        (None, true, RotateHead::ID),
    ];

    for (wire_move, send_head_rotation, expected_id) in cases {
        let mut movement = movement(wire_move);
        movement.send_head_rotation = send_head_rotation;
        let mut writer = Vec::new();

        send_entity_relative_move(&mut writer, Compression::Disabled, &movement)
            .await
            .unwrap();

        assert_eq!(decode_packet_ids(&writer), vec![expected_id]);
    }
}

#[tokio::test]
async fn motion_precedes_relative_movement_and_optional_head_rotation() {
    let mut movement = movement(Some(ServerEntityWireMove::PositionRotation {
        delta: Vec3::new(0.25, 0.0, -0.5),
    }));
    movement.send_velocity = true;
    movement.send_head_rotation = true;
    let mut writer = Vec::new();

    send_entity_relative_move(&mut writer, Compression::Disabled, &movement)
        .await
        .unwrap();

    assert_eq!(
        decode_packet_ids(&writer),
        vec![SetEntityMotion::ID, MoveEntityPosRot::ID, RotateHead::ID,]
    );
}

#[tokio::test]
async fn velocity_and_absolute_sync_keep_oracle_packet_order() {
    let mut movement = movement(Some(ServerEntityWireMove::Absolute {
        position: Vec3::new(9.0, 64.0, -3.0),
    }));
    movement.send_velocity = true;
    movement.send_head_rotation = true;
    let mut writer = Vec::new();

    send_entity_relative_move(&mut writer, Compression::Disabled, &movement)
        .await
        .unwrap();

    assert_eq!(
        decode_packet_ids(&writer),
        vec![SetEntityMotion::ID, EntityPositionSync::ID, RotateHead::ID,]
    );

    let mut bytes = BytesMut::from(writer.as_slice());
    let _motion = try_decode_frame(&mut bytes, Compression::Disabled)
        .unwrap()
        .expect("motion frame");
    let mut absolute = try_decode_frame(&mut bytes, Compression::Disabled)
        .unwrap()
        .expect("position sync frame");
    assert_eq!(
        EntityPositionSync::decode(&mut absolute.body).unwrap(),
        EntityPositionSync {
            entity_id: 17,
            values: PositionMoveRotation {
                position: EntityVec3 {
                    x: 9.0,
                    y: 64.0,
                    z: -3.0,
                },
                delta_movement: EntityVec3 {
                    x: 0.05,
                    y: 0.0,
                    z: -0.1,
                },
                yaw: 90.0,
                pitch: -15.0,
            },
            on_ground: false,
        }
    );
}

#[tokio::test]
async fn velocity_only_update_emits_only_motion() {
    let mut movement = movement(None);
    movement.send_velocity = true;
    let mut writer = Vec::new();

    send_entity_relative_move(&mut writer, Compression::Disabled, &movement)
        .await
        .unwrap();

    assert_eq!(decode_packet_ids(&writer), vec![SetEntityMotion::ID]);
}
