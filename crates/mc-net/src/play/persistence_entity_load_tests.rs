use std::io::Write;

use mc_entity::RegionalDecisionJournal;

use super::*;

fn items() -> ItemRegistry {
    ItemRegistry::from_report(&[mc_data::items::ItemReport {
        id: mc_data::Identifier::parse("minecraft:stone").unwrap(),
        protocol_id: 1,
    }])
}

fn entity_types() -> EntityTypeRegistry {
    mc_data::entity_types::solaris_required_entity_types()
}

fn snapshot(id: i32, lifecycle: EntityLifecycle) -> EntitySnapshot {
    EntitySnapshot {
        id: EntityId(id),
        uuid: uuid::Uuid::from_u128(id as u128),
        type_id: 71,
        type_name: "minecraft:item".into(),
        position: Vec3::new(1.0, 65.0, 2.0),
        rotation: mc_entity::Rotation {
            yaw: 15.0,
            pitch: -4.0,
            head_yaw: 15.0,
        },
        velocity: Vec3::new(0.1, 0.2, 0.3),
        on_ground: false,
        item_stack: Some(EntityItemStack::new(1, 3)),
        experience_value: None,
        block_state: None,
        lifecycle,
        health: 20.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: GoalState::Idle,
        vehicle: None,
        animal: None,
        retained: mc_entity::EntityRetainedState::default(),
    }
}

fn persisted_entity(id: i32, uuid: uuid::Uuid) -> Tag {
    let mut retained = mc_entity::EntityRetainedState::default();
    retained.spawn_tick = 77;
    retained.item_pickup_ready_tick = Some(207);
    Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:item".into())),
        ("SolarisEntityId".into(), Tag::Int(id)),
        ("UUID".into(), Tag::IntArray(uuid_to_int_array(uuid))),
        ("Pos".into(), double_list_tag([1.25, 65.5, -2.75])),
        ("Motion".into(), double_list_tag([0.1, -0.2, 0.3])),
        (
            "Rotation".into(),
            Tag::List(ListTag {
                element_type: tag_type::FLOAT,
                elements: vec![Tag::Float(15.0), Tag::Float(-4.0)],
            }),
        ),
        ("OnGround".into(), Tag::Byte(0)),
        ("Health".into(), Tag::Float(20.0)),
        (
            ENTITY_ATTRIBUTES_FIELD.into(),
            Tag::String(
                serde_json::to_string(&mc_entity::AttributeSet::vanilla_mob_defaults()).unwrap(),
            ),
        ),
        (ENTITY_LIFECYCLE_FIELD.into(), Tag::Byte(0)),
        (
            ENTITY_RETAINED_STATE_FIELD.into(),
            Tag::String(serde_json::to_string(&retained).unwrap()),
        ),
        (ENTITY_HEAD_YAW_FIELD.into(), Tag::Float(15.0)),
        (
            ENTITY_GOAL_STATE_FIELD.into(),
            Tag::String(serde_json::to_string(&GoalState::Idle).unwrap()),
        ),
        (
            ENTITY_VEHICLE_STATE_FIELD.into(),
            Tag::String(serde_json::to_string(&Option::<mc_entity::VehicleState>::None).unwrap()),
        ),
        ("Age".into(), Tag::Int(123)),
        (
            "Item".into(),
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:stone".into())),
                ("count".into(), Tag::Int(3)),
            ]),
        ),
        ("PickupDelay".into(), Tag::Short(7)),
    ])
}

fn double_list_tag(values: [f64; 3]) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::DOUBLE,
        elements: values.into_iter().map(Tag::Double).collect(),
    })
}

fn entities_root(entities: Vec<Tag>) -> Tag {
    Tag::Compound(vec![(
        "Entities".into(),
        Tag::List(ListTag {
            element_type: if entities.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: entities,
        }),
    )])
}

fn versioned_entities_root(version: i32, entities: Vec<Tag>) -> Tag {
    let Tag::Compound(mut fields) = entities_root(entities) else {
        unreachable!("entities_root always returns a compound");
    };
    fields.push(("SolarisEntityFormatVersion".into(), Tag::Int(version)));
    fields.push(("SolarisEntityLifecycleTick".into(), Tag::Long(200)));
    fields.push(("SolarisRegionalSequenceWatermark".into(), Tag::Long(0)));
    Tag::Compound(fields)
}

fn write_existing_gzip_file(world_root: &Path, root: &Tag) {
    let path = entities_path(world_root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    let mut nbt = Vec::new();
    mc_nbt::write_named(&mut nbt, "", root).unwrap();
    let file = File::create(path).unwrap();
    let mut encoder = GzEncoder::new(file, GzipCompression::default());
    encoder.write_all(&nbt).unwrap();
    encoder.finish().unwrap();
}

fn set_entity_field(entity: &mut Tag, name: &str, value: Tag) {
    let Tag::Compound(fields) = entity else {
        panic!("entity fixture must be a compound");
    };
    set_field(fields, name, value);
}

fn remove_entity_field(entity: &mut Tag, name: &str) {
    let Tag::Compound(fields) = entity else {
        panic!("entity fixture must be a compound");
    };
    fields.retain(|(field, _)| field != name);
}

#[test]
fn missing_entity_format_version_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let uuid = uuid::Uuid::from_u128(0x1234);
    write_existing_gzip_file(tmp.path(), &entities_root(vec![persisted_entity(41, uuid)]));

    let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .expect_err("entity persistence requires an explicit format version");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: ENTITY_FORMAT_VERSION_FIELD,
            ..
        }
    ));
}

#[test]
fn missing_lifecycle_clock_fails_closed_even_for_an_empty_v2_checkpoint() {
    let tmp = tempfile::tempdir().unwrap();
    let Tag::Compound(mut fields) = entities_root(Vec::new()) else {
        unreachable!("entities root is a compound");
    };
    fields.push((
        "SolarisEntityFormatVersion".into(),
        Tag::Int(ENTITY_FORMAT_VERSION),
    ));
    write_existing_gzip_file(tmp.path(), &Tag::Compound(fields));

    let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .expect_err("v2 requires one authoritative lifecycle clock");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: ENTITY_LIFECYCLE_TICK_FIELD,
            ..
        }
    ));
}

#[test]
fn missing_regional_sequence_watermark_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let Tag::Compound(mut fields) = versioned_entities_root(ENTITY_FORMAT_VERSION, Vec::new())
    else {
        unreachable!("versioned entities root is a compound");
    };
    fields.retain(|(name, _)| name != ENTITY_REGIONAL_SEQUENCE_FIELD);
    write_existing_gzip_file(tmp.path(), &Tag::Compound(fields));

    let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .expect_err("format 3 requires one regional sequence watermark");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: ENTITY_REGIONAL_SEQUENCE_FIELD,
            ..
        }
    ));
}

#[test]
fn empty_checkpoint_round_trips_its_authoritative_lifecycle_clock() {
    let tmp = tempfile::tempdir().unwrap();
    save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new(777, Vec::<PersistedEntityRecord>::new()),
    )
    .unwrap();

    let loaded = load_persisted_entities(tmp.path(), &items(), &entity_types()).unwrap();
    assert_eq!(loaded.lifecycle_clock, 777);
    assert!(loaded.records.is_empty());
}

#[test]
fn retained_wander_state_loads_before_pause_fields_existed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entity = persisted_entity(42, uuid::Uuid::from_u128(42));
    let mut retained_state = mc_entity::EntityRetainedState::default();
    retained_state.spawn_tick = 77;
    retained_state.item_pickup_ready_tick = Some(207);
    let mut retained = serde_json::to_value(retained_state).unwrap();
    let path = retained["path"]
        .as_object_mut()
        .expect("retained path is a JSON object");
    path.remove("target_reached");
    path.remove("resume_tick");
    serde_json::from_value::<mc_entity::EntityRetainedState>(retained.clone())
        .expect("legacy retained path fields use defaults");
    set_entity_field(
        &mut entity,
        ENTITY_RETAINED_STATE_FIELD,
        Tag::String(serde_json::to_string(&retained).unwrap()),
    );
    write_existing_gzip_file(
        tmp.path(),
        &versioned_entities_root(ENTITY_FORMAT_VERSION, vec![entity]),
    );

    let loaded = load_persisted_entities(tmp.path(), &items(), &entity_types()).unwrap();

    assert_eq!(loaded.records.len(), 1);
    assert_eq!(loaded.records[0].snapshot.id, EntityId(42));
    assert_eq!(loaded.records[0].snapshot.retained.spawn_tick, 77);
    assert_eq!(
        loaded.records[0].snapshot.retained.item_pickup_ready_tick,
        Some(207)
    );
    let retained = serde_json::to_value(&loaded.records[0].snapshot.retained).unwrap();
    assert_eq!(retained["path"]["target_reached"], false);
    assert_eq!(retained["path"]["resume_tick"], 0);
}

#[test]
fn lifecycle_clock_overflow_fails_closed_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let error = save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new(i64::MAX as u64 + 1, Vec::<PersistedEntityRecord>::new()),
    )
    .expect_err("NBT long cannot represent an overflowing lifecycle clock");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: ENTITY_LIFECYCLE_TICK_FIELD,
            ..
        }
    ));
    assert!(!entities_path(tmp.path()).exists());
}

#[test]
fn regional_sequence_overflow_fails_closed_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let error = save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new_at_owner_sequence(
            1,
            i64::MAX as u64 + 1,
            Vec::<PersistedEntityRecord>::new(),
        ),
    )
    .expect_err("NBT long cannot represent an overflowing regional sequence");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: ENTITY_REGIONAL_SEQUENCE_FIELD,
            ..
        }
    ));
    assert!(!entities_path(tmp.path()).exists());
}

#[test]
fn impossible_future_retained_ticks_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut future_damage = PersistedEntityRecord::from(snapshot(80, EntityLifecycle::Alive));
    future_damage.snapshot.retained.last_damage_tick = Some(21);
    let mut distant_death = PersistedEntityRecord::from(snapshot(81, EntityLifecycle::Despawning));
    distant_death.snapshot.retained.death_remove_tick = Some(41);
    let mut invalid_grazing = PersistedEntityRecord::from(snapshot(82, EntityLifecycle::Alive));
    invalid_grazing.snapshot.retained.sheep_grazing_ticks = Some(41);

    for record in [future_damage, distant_death, invalid_grazing] {
        let error = save_persisted_entity_records(
            tmp.path(),
            &items(),
            &PersistedEntityCheckpoint::new(20, vec![record]),
        )
        .expect_err("impossible retained temporal state must not be persisted");
        assert!(matches!(
            error,
            PlayerPersistenceError::InvalidValue {
                field: ENTITY_RETAINED_STATE_FIELD,
                ..
            }
        ));
    }
}

#[test]
fn current_gzip_entity_file_restarts_with_the_existing_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let mut snapshot = snapshot(42, EntityLifecycle::Alive);
    snapshot.retained.item_pickup_ready_tick = Some(330);
    let record = PersistedEntityRecord {
        snapshot,
        age: 321,
        pickup_delay: 9,
    };
    save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new_at_owner_sequence(321, 444, vec![record.clone()]),
    )
    .unwrap();

    let bytes = std::fs::read(entities_path(tmp.path())).unwrap();
    assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
    let (root_name, root) = read_player_root(&entities_path(tmp.path())).unwrap();
    assert!(root_name.is_empty());
    let Tag::Compound(fields) = root else {
        panic!("entity root must be a compound");
    };
    assert_eq!(fields.len(), 4);
    assert!(matches!(field(&fields, "Entities"), Some(Tag::List(_))));
    assert_eq!(
        int_field(&fields, "SolarisEntityFormatVersion"),
        Some(ENTITY_FORMAT_VERSION)
    );
    assert_eq!(long_field(&fields, "SolarisEntityLifecycleTick"), Some(321));
    assert_eq!(
        long_field(&fields, "SolarisRegionalSequenceWatermark"),
        Some(444)
    );
    assert!(field(&fields, "DataVersion").is_none());
    let Some(Tag::List(entities)) = field(&fields, "Entities") else {
        panic!("entity root contains a list");
    };
    let Tag::Compound(entity_fields) = &entities.elements[0] else {
        panic!("persisted entity is a compound");
    };
    assert_eq!(
        double_list::<3>(field(entity_fields, "Motion").unwrap(), 3),
        Some([0.005, 0.01, 0.015])
    );

    let loaded = load_persisted_entities(tmp.path(), &items(), &entity_types()).unwrap();
    assert_eq!(loaded.lifecycle_clock, 321);
    assert_eq!(loaded.regional_sequence_watermark, 444);
    let loaded = loaded.records;
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].uuid, record.uuid);
    assert_eq!(loaded[0].age, 321);
    assert_eq!(loaded[0].pickup_delay, 9);
    assert_eq!(loaded[0].velocity, record.velocity);
}

#[test]
fn checkpoint_round_trip_preserves_head_yaw_goal_and_vehicle_graph() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vehicle = PersistedEntityRecord::from(snapshot(45, EntityLifecycle::Alive));
    vehicle.snapshot.type_id = 30;
    vehicle.snapshot.type_name = "minecraft:cow".into();
    vehicle.snapshot.item_stack = None;
    vehicle.snapshot.animal = Some(mc_entity::AnimalBreedingState::adult());
    vehicle.snapshot.rotation = mc_entity::Rotation {
        yaw: 10.0,
        pitch: 20.0,
        head_yaw: 70.0,
    };
    vehicle.snapshot.goal = GoalState::FollowPosition {
        target: Vec3::new(8.5, 64.0, -3.5),
        speed: 0.35,
    };
    vehicle.snapshot.vehicle = Some(mc_entity::VehicleState {
        kind: mc_entity::VehicleKind::Boat,
        passenger: Some(EntityId(46)),
    });
    let mut passenger = PersistedEntityRecord::from(snapshot(46, EntityLifecycle::Alive));
    passenger.snapshot.type_id = 30;
    passenger.snapshot.type_name = "minecraft:cow".into();
    passenger.snapshot.item_stack = None;
    passenger.snapshot.animal = Some(mc_entity::AnimalBreedingState::adult());

    save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new(0, [vehicle.clone(), passenger.clone()]),
    )
    .unwrap();
    let loaded = load_persisted_entities(tmp.path(), &items(), &entity_types()).unwrap();

    assert_eq!(loaded.records.len(), 2);
    assert_eq!(loaded.records[0].snapshot, vehicle.snapshot);
    assert_eq!(loaded.records[1].snapshot, passenger.snapshot);
}

#[test]
fn checkpoint_load_requires_each_authoritative_projection_field() {
    for field in [
        ENTITY_HEAD_YAW_FIELD,
        ENTITY_GOAL_STATE_FIELD,
        ENTITY_VEHICLE_STATE_FIELD,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let mut entity = persisted_entity(45, uuid::Uuid::from_u128(45));
        remove_entity_field(&mut entity, field);
        write_existing_gzip_file(
            tmp.path(),
            &versioned_entities_root(ENTITY_FORMAT_VERSION, vec![entity]),
        );

        let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
            .expect_err("format 3 must fail closed when an authoritative field is absent");
        assert!(matches!(
            error,
            PlayerPersistenceError::InvalidValue {
                field: invalid_field,
                ..
            } if invalid_field == field
        ));
    }
}

#[test]
fn checkpoint_rejects_dangling_vehicle_graph_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vehicle = PersistedEntityRecord::from(snapshot(47, EntityLifecycle::Alive));
    vehicle.snapshot.vehicle = Some(mc_entity::VehicleState {
        kind: mc_entity::VehicleKind::Boat,
        passenger: Some(EntityId(48)),
    });

    let error = save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new(0, [vehicle]),
    )
    .expect_err("dangling vehicle graph must fail before checkpoint replacement");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: ENTITY_VEHICLE_STATE_FIELD,
            ..
        }
    ));
    assert!(!entities_path(tmp.path()).exists());
}

#[test]
fn damaged_restart_preserves_independent_max_health_for_later_healing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut record = PersistedEntityRecord::from(snapshot(44, EntityLifecycle::Alive));
    record.snapshot.type_id = 30;
    record.snapshot.type_name = "minecraft:cow".into();
    record.snapshot.item_stack = None;
    record.snapshot.health = 7.0;
    record
        .snapshot
        .attributes
        .set_base(AttributeKind::MaxHealth, 32.0);

    save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new(0, vec![record.clone()]),
    )
    .unwrap();
    let loaded = load_persisted_entities(tmp.path(), &items(), &entity_types()).unwrap();

    assert_eq!(loaded.records[0].health, 7.0);
    let max_health = loaded.records[0]
        .attributes
        .base(&AttributeKind::MaxHealth)
        .expect("restored living entity retains max health") as f32;
    assert_eq!(max_health, 32.0);
    assert!(loaded.records[0].health < max_health);

    let registry = SessionRegistry::new();
    assert_eq!(registry.restore_persisted_entities(loaded), 1);
    let restored = registry.persisted_entity_records()[0].snapshot.clone();
    let (result, dispatches) = registry.apply_server_entity_effect_request(
        &crate::play::simulation::SimulationAuthority::for_test(),
        Some(restored.clone()),
        restored.id,
        mc_entity::EntityEffectRequest {
            operation: mc_entity::EntityEffectOperation::ApplyAction {
                effect_id: mc_entity::effects_26_1_2::EffectId::new(0),
                action: mc_entity::runtime_26_1_2::EffectAction::Heal { amount: 100.0 },
                damage_context: None,
            },
            target_kind: mc_entity::runtime_26_1_2::TargetKind::NonPlayer,
            death_remove_tick: 20,
        },
    );
    let mc_entity::EntityEffectResult::Applied(healed) = result else {
        panic!("restored damaged entity must accept healing");
    };
    assert_eq!(dispatches.len(), 0);
    assert_eq!(healed.snapshot.health, max_health);
    assert_eq!(
        healed.snapshot.attributes.base(&AttributeKind::MaxHealth),
        Some(f64::from(max_health))
    );
}

#[test]
fn loaded_entity_position_and_velocity_are_normalized_at_oracle_limits() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entity = persisted_entity(43, uuid::Uuid::from_u128(43));
    set_entity_field(
        &mut entity,
        "Pos",
        double_list_tag([40_000_000.0, -30_000_000.0, -40_000_000.0]),
    );
    set_entity_field(
        &mut entity,
        "Motion",
        double_list_tag([10.0, -10.000_001, 11.0]),
    );
    write_existing_gzip_file(
        tmp.path(),
        &versioned_entities_root(ENTITY_FORMAT_VERSION, vec![entity]),
    );

    let loaded = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .unwrap()
        .records;

    assert_eq!(
        loaded[0].position,
        Vec3::new(30_000_512.0, -20_000_000.0, -30_000_512.0)
    );
    assert_eq!(loaded[0].velocity, Vec3::new(200.0, 0.0, 0.0));
}

#[test]
fn unsupported_entity_format_versions_fail_closed() {
    for version in [1, 2, 99] {
        let tmp = tempfile::tempdir().unwrap();
        write_existing_gzip_file(
            tmp.path(),
            &versioned_entities_root(
                version,
                vec![persisted_entity(50, uuid::Uuid::from_u128(50))],
            ),
        );

        let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
            .expect_err("unsupported entity persistence versions must fail closed");

        assert!(matches!(
            error,
            PlayerPersistenceError::UnsupportedEntityFormatVersion {
                version: actual,
                ..
            } if actual == version
        ));
    }
}

#[test]
fn malformed_entity_format_version_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut root = versioned_entities_root(
        ENTITY_FORMAT_VERSION,
        vec![persisted_entity(51, uuid::Uuid::from_u128(51))],
    );
    let Tag::Compound(fields) = &mut root else {
        unreachable!("versioned_entities_root always returns a compound");
    };
    set_field(
        fields,
        "SolarisEntityFormatVersion",
        Tag::String("2".into()),
    );
    write_existing_gzip_file(tmp.path(), &root);

    let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .expect_err("malformed entity persistence versions must fail closed");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: "SolarisEntityFormatVersion",
            ..
        }
    ));
}

#[test]
fn duplicate_entity_format_version_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut root = versioned_entities_root(
        ENTITY_FORMAT_VERSION,
        vec![persisted_entity(52, uuid::Uuid::from_u128(52))],
    );
    let Tag::Compound(fields) = &mut root else {
        unreachable!("versioned_entities_root always returns a compound");
    };
    fields.push(("SolarisEntityFormatVersion".into(), Tag::Int(99)));
    write_existing_gzip_file(tmp.path(), &root);

    let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .expect_err("duplicate entity persistence versions must fail closed");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: "SolarisEntityFormatVersion",
            ..
        }
    ));
}

#[test]
fn loaded_entity_non_finite_position_rotation_and_velocity_fail_closed() {
    let cases = [
        ("Pos", double_list_tag([f64::NAN, 65.0, 2.0]), "Pos"),
        (
            "Rotation",
            Tag::List(ListTag {
                element_type: tag_type::FLOAT,
                elements: vec![Tag::Float(f32::INFINITY), Tag::Float(0.0)],
            }),
            "Rotation",
        ),
        (
            "Motion",
            double_list_tag([0.0, f64::NEG_INFINITY, 0.0]),
            "Motion",
        ),
    ];

    for (field_name, malformed, expected_field) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let mut entity = persisted_entity(44, uuid::Uuid::from_u128(44));
        set_entity_field(&mut entity, field_name, malformed);
        write_existing_gzip_file(
            tmp.path(),
            &versioned_entities_root(ENTITY_FORMAT_VERSION, vec![entity]),
        );

        let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
            .expect_err("non-finite entity kinematics must fail closed");
        assert!(matches!(
            error,
            PlayerPersistenceError::InvalidNumeric { field, .. } if field == expected_field
        ));
    }
}

#[test]
fn duplicate_uuid_rejects_the_entire_staged_entity_load() {
    let tmp = tempfile::tempdir().unwrap();
    let duplicate = uuid::Uuid::from_u128(0xfeed);
    write_existing_gzip_file(
        tmp.path(),
        &versioned_entities_root(
            ENTITY_FORMAT_VERSION,
            vec![
                persisted_entity(45, duplicate),
                persisted_entity(46, duplicate),
            ],
        ),
    );

    let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .expect_err("a duplicate UUID must reject the staged load, not return a prefix");
    assert!(error.to_string().contains("duplicate entity UUID"));
}

#[test]
fn entity_save_round_trips_every_reachable_lifecycle_state() {
    let tmp = tempfile::tempdir().unwrap();
    let mut alive = PersistedEntityRecord::from(snapshot(47, EntityLifecycle::Alive));
    alive.snapshot.retained.spawn_tick = 20;
    alive.snapshot.retained.last_damage_tick = Some(17);
    alive.snapshot.retained.sheep_grazing_ticks = Some(4);
    let mut despawning = PersistedEntityRecord::from(snapshot(48, EntityLifecycle::Despawning));
    despawning.snapshot.retained.spawn_tick = 20;
    despawning.snapshot.retained.last_damage_tick = Some(19);
    despawning.snapshot.retained.death_remove_tick = Some(39);
    let alive_retained = alive.snapshot.retained.clone();
    let despawning_retained = despawning.snapshot.retained.clone();

    save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new(20, vec![alive, despawning]),
    )
    .unwrap();

    let loaded = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .unwrap()
        .records;
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].id, EntityId(47));
    assert_eq!(loaded[0].lifecycle, EntityLifecycle::Alive);
    assert_eq!(loaded[0].retained, alive_retained);
    assert_eq!(loaded[1].id, EntityId(48));
    assert_eq!(loaded[1].lifecycle, EntityLifecycle::Despawning);
    assert_eq!(loaded[1].retained, despawning_retained);
}

#[test]
fn restart_round_trips_living_clocks_and_hidden_effect_chains_exactly() {
    use mc_entity::effects_26_1_2::{
        ActiveEffectChainSnapshot, ActiveEffectsSnapshot, EffectFlags, EffectId, EffectInstance,
        EffectKind,
    };

    let tmp = tempfile::tempdir().unwrap();
    let mut record = PersistedEntityRecord::from(snapshot(49, EntityLifecycle::Alive));
    record.snapshot.retained.living = mc_entity::EntityLivingRetainedState {
        absorption: 2.5,
        invulnerable_time: 14,
        hurt_time: 7,
        last_hurt: 3.0,
        death_time: 0,
    };
    let effect_id = EffectId::new(10);
    record.snapshot.retained.active_effects = Some(mc_entity::EntityActiveEffectsState {
        effects: ActiveEffectsSnapshot {
            chains: vec![ActiveEffectChainSnapshot {
                current: EffectInstance::new(
                    effect_id,
                    EffectKind::Regeneration,
                    40,
                    1,
                    EffectFlags::default(),
                ),
                hidden: vec![EffectInstance::new(
                    effect_id,
                    EffectKind::Regeneration,
                    180,
                    0,
                    EffectFlags::default(),
                )],
            }],
        },
        action_order: vec![effect_id],
    });
    let expected = record.snapshot.clone();
    save_persisted_entity_records(
        tmp.path(),
        &items(),
        &PersistedEntityCheckpoint::new(0, [record]),
    )
    .unwrap();

    let checkpoint = load_persisted_entities(tmp.path(), &items(), &entity_types()).unwrap();
    assert_eq!(checkpoint.records[0].snapshot, expected);
    let registry = SessionRegistry::new();
    assert_eq!(registry.restore_persisted_entities(checkpoint), 1);
    assert_eq!(
        registry.authoritative_entity_snapshot(EntityId(49)),
        Some(expected)
    );
}

#[test]
fn missing_or_malformed_authoritative_attributes_fail_closed() {
    for attributes in [None, Some(Tag::String("not json".into()))] {
        let tmp = tempfile::tempdir().unwrap();
        let mut entity = persisted_entity(53, uuid::Uuid::from_u128(53));
        let Tag::Compound(fields) = &mut entity else {
            unreachable!("persisted_entity always returns a compound");
        };
        fields.retain(|(name, _)| name != ENTITY_ATTRIBUTES_FIELD);
        if let Some(attributes) = attributes {
            fields.push((ENTITY_ATTRIBUTES_FIELD.into(), attributes));
        }
        write_existing_gzip_file(
            tmp.path(),
            &versioned_entities_root(ENTITY_FORMAT_VERSION, vec![entity]),
        );

        let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
            .expect_err("schema v2 requires valid authoritative attributes");
        assert!(matches!(
            error,
            PlayerPersistenceError::InvalidValue {
                field: ENTITY_ATTRIBUTES_FIELD,
                ..
            }
        ));
    }
}

#[test]
fn removed_lifecycle_code_is_rejected_by_schema_v2() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entity = persisted_entity(49, uuid::Uuid::from_u128(49));
    set_entity_field(&mut entity, ENTITY_LIFECYCLE_FIELD, Tag::Byte(2));
    write_existing_gzip_file(
        tmp.path(),
        &versioned_entities_root(ENTITY_FORMAT_VERSION, vec![entity]),
    );

    let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
        .expect_err("removed entities must be absent, not encoded in schema v2");

    assert!(matches!(
        error,
        PlayerPersistenceError::InvalidValue {
            field: ENTITY_LIFECYCLE_FIELD,
            ..
        }
    ));
}

fn journal_path(world_root: &Path) -> PathBuf {
    world_root
        .join(SOLARIS_DIR)
        .join(REGIONAL_DECISION_JOURNAL_FILE)
}

fn decision(phase: u64, sequence: u64) -> RegionalCommitDecision {
    RegionalCommitDecision::from_parts(RegionPhase(phase), sequence, Vec::new(), Vec::new())
        .expect("test decision")
}

fn framed_wal_record(payload: &[u8]) -> Vec<u8> {
    let payload_len = u32::try_from(payload.len()).expect("test payload fits a WAL record");
    let mut frame = Vec::with_capacity(8 + payload.len());
    frame.extend_from_slice(&payload_len.to_be_bytes());
    frame.extend_from_slice(&crc32fast::hash(payload).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn framed_wal_group(decisions: &[RegionalCommitDecision]) -> Vec<u8> {
    framed_wal_record(&serde_json::to_vec(decisions).expect("serialize test WAL group"))
}

fn write_framed_wal(world_root: &Path, frames: impl IntoIterator<Item = Vec<u8>>) {
    let path = journal_path(world_root);
    std::fs::create_dir_all(path.parent().expect("journal path has parent")).unwrap();
    let mut bytes = REGIONAL_DECISION_JOURNAL_HEADER.to_vec();
    for frame in frames {
        bytes.extend_from_slice(&frame);
    }
    std::fs::write(&path, bytes).unwrap();
}

fn open_journal_error(world_root: &Path) -> RegionalDecisionJournalOpenError {
    match FileRegionalDecisionJournal::open(world_root) {
        Ok(_) => panic!("regional decision journal unexpectedly opened"),
        Err(error) => error,
    }
}

fn malformed_decision_group(mut decision: serde_json::Value) -> Vec<u8> {
    framed_wal_record(
        &serde_json::to_vec(&vec![decision.take()]).expect("serialize malformed decision group"),
    )
}

#[test]
fn regional_decision_journal_rejects_trailing_complete_junk() {
    let tmp = tempfile::tempdir().unwrap();
    let path = journal_path(tmp.path());
    let (mut journal, pending) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    assert!(pending.is_empty());
    journal.record_commit(&decision(1, 1)).unwrap();
    drop(journal);

    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(&framed_wal_record(b"complete junk"));
    std::fs::write(path, bytes).unwrap();

    open_journal_error(tmp.path());
}

#[test]
fn regional_decision_journal_recovers_empty_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let path = journal_path(tmp.path());
    write_framed_wal(tmp.path(), []);

    let (journal, pending) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    assert!(pending.is_empty());
    drop(journal);
    assert_eq!(
        std::fs::read(path).unwrap(),
        REGIONAL_DECISION_JOURNAL_HEADER
    );
}

#[test]
fn regional_decision_journal_recovers_frame_header_prefix() {
    let frame = framed_wal_group(&[decision(1, 1)]);
    for prefix_len in 1..REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES {
        let tmp = tempfile::tempdir().unwrap();
        let path = journal_path(tmp.path());
        write_framed_wal(tmp.path(), [frame[..prefix_len].to_vec()]);

        let (journal, pending) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert!(pending.is_empty());
        drop(journal);
        assert_eq!(
            std::fs::read(path).unwrap(),
            REGIONAL_DECISION_JOURNAL_HEADER
        );
    }
}

#[test]
fn regional_decision_journal_recovers_frame_payload_prefix() {
    let frame = framed_wal_group(&[decision(1, 1)]);
    for prefix_len in [
        REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES,
        REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES + 1,
        frame.len() - 1,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let path = journal_path(tmp.path());
        write_framed_wal(tmp.path(), [frame[..prefix_len].to_vec()]);

        let (journal, pending) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert!(pending.is_empty());
        drop(journal);
        assert_eq!(
            std::fs::read(path).unwrap(),
            REGIONAL_DECISION_JOURNAL_HEADER
        );
    }
}

#[test]
fn regional_decision_journal_recovers_exact_complete_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let path = journal_path(tmp.path());
    let first = decision(1, 1);
    let second_frame = framed_wal_group(&[decision(2, 2)]);
    write_framed_wal(
        tmp.path(),
        [
            framed_wal_group(std::slice::from_ref(&first)),
            second_frame[..second_frame.len() - 1].to_vec(),
        ],
    );

    let (journal, pending) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    assert_eq!(pending, vec![first.clone()]);
    drop(journal);
    let expected = [
        REGIONAL_DECISION_JOURNAL_HEADER,
        framed_wal_group(&[first]).as_slice(),
    ]
    .concat();
    assert_eq!(std::fs::read(path).unwrap(), expected);
}

#[test]
fn regional_decision_journal_rejects_bit_flipped_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let path = journal_path(tmp.path());
    let (mut journal, _) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    journal.record_commit(&decision(1, 1)).unwrap();
    drop(journal);

    let mut bytes = std::fs::read(&path).unwrap();
    let phase = b"\"phase\":1";
    let phase_offset = bytes
        .windows(phase.len())
        .position(|window| window == phase)
        .expect("serialized decision contains its phase");
    bytes[phase_offset + phase.len() - 1] = b'2';
    std::fs::write(path, bytes).unwrap();

    assert!(matches!(
        open_journal_error(tmp.path()),
        RegionalDecisionJournalOpenError::Checksum { .. }
    ));
}

#[test]
fn regional_decision_journal_rejects_checksum_valid_malformed_json() {
    let tmp = tempfile::tempdir().unwrap();
    write_framed_wal(tmp.path(), [framed_wal_record(b"not JSON")]);

    assert!(matches!(
        open_journal_error(tmp.path()),
        RegionalDecisionJournalOpenError::Json { .. }
    ));
}

#[test]
fn regional_decision_journal_rejects_empty_group() {
    let tmp = tempfile::tempdir().unwrap();
    write_framed_wal(tmp.path(), [framed_wal_group(&[])]);

    assert!(matches!(
        open_journal_error(tmp.path()),
        RegionalDecisionJournalOpenError::Framing {
            reason: "empty commit group",
            ..
        }
    ));
}

#[test]
fn regional_decision_journal_reconstructs_decisions_through_constructor() {
    let upsert = snapshot(80, EntityLifecycle::Alive);
    let upsert_id = upsert.id;
    let valid_upsert =
        RegionalCommitDecision::from_parts(RegionPhase(1), 1, vec![upsert], Vec::new()).unwrap();
    let valid_removed =
        RegionalCommitDecision::from_parts(RegionPhase(1), 1, Vec::new(), vec![upsert_id]).unwrap();

    let mut duplicate_upsert = serde_json::to_value(&valid_upsert).unwrap();
    let upserts = duplicate_upsert["upserts"].as_array_mut().unwrap();
    upserts.push(upserts[0].clone());

    let mut duplicate_removal = serde_json::to_value(&valid_removed).unwrap();
    let removed = duplicate_removal["removed"].as_array_mut().unwrap();
    removed.push(removed[0].clone());

    let mut overlap = serde_json::to_value(&valid_upsert).unwrap();
    overlap["removed"] = serde_json::json!([upsert_id]);

    for malformed in [duplicate_upsert, duplicate_removal, overlap] {
        let tmp = tempfile::tempdir().unwrap();
        write_framed_wal(tmp.path(), [malformed_decision_group(malformed)]);

        assert!(matches!(
            open_journal_error(tmp.path()),
            RegionalDecisionJournalOpenError::Validation { .. }
        ));
    }
}

#[test]
fn regional_decision_journal_rejects_non_monotonic_or_duplicate_ordering() {
    for decisions in [
        vec![decision(2, 2), decision(1, 3)],
        vec![decision(2, 2), decision(2, 3)],
        vec![decision(2, 2), decision(3, 2)],
    ] {
        let tmp = tempfile::tempdir().unwrap();
        write_framed_wal(tmp.path(), [framed_wal_group(&decisions)]);

        assert!(matches!(
            open_journal_error(tmp.path()),
            RegionalDecisionJournalOpenError::Validation { .. }
        ));
    }
}

#[test]
fn regional_decision_journal_rejects_regressing_lifecycle_epoch() {
    let decisions = vec![
        RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            RegionPhase(1),
            1,
            10,
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            RegionPhase(2),
            2,
            9,
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
    ];
    let tmp = tempfile::tempdir().unwrap();
    write_framed_wal(tmp.path(), [framed_wal_group(&decisions)]);

    assert!(matches!(
        open_journal_error(tmp.path()),
        RegionalDecisionJournalOpenError::Validation { .. }
    ));
}

#[test]
fn replay_returns_invalid_data_for_non_monotonic_or_duplicate_ordering() {
    for decisions in [
        vec![decision(2, 2), decision(1, 3)],
        vec![decision(2, 2), decision(2, 3)],
        vec![decision(2, 2), decision(3, 2)],
    ] {
        let error = replay_regional_commit_decisions(
            PersistedEntityCheckpoint::new(0, Vec::<PersistedEntityRecord>::new()),
            &decisions,
        )
        .unwrap_err();
        let error = std::io::Error::from(error);
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}

#[test]
fn replay_returns_invalid_data_for_duplicate_final_uuid() {
    let original = PersistedEntityRecord::from(snapshot(71, EntityLifecycle::Alive));
    let mut duplicate = snapshot(72, EntityLifecycle::Alive);
    duplicate.uuid = original.snapshot.uuid;
    let decision =
        RegionalCommitDecision::from_parts(RegionPhase(1), 1, vec![duplicate], Vec::new()).unwrap();

    let error = replay_regional_commit_decisions(
        PersistedEntityCheckpoint::new(0, vec![original]),
        &[decision],
    )
    .unwrap_err();
    let error = std::io::Error::from(error);
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn replay_rejects_newer_epoch_that_regresses_global_sequence_watermark() {
    let checkpoint = PersistedEntityCheckpoint::new_at_owner_sequence(
        100,
        1_000,
        Vec::<PersistedEntityRecord>::new(),
    );
    let decision = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
        RegionPhase(1),
        900,
        101,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();

    let error = replay_regional_commit_decisions(checkpoint, &[decision]).unwrap_err();

    assert!(matches!(
        error,
        RegionalDecisionReplayError::InvalidOrdering
    ));
    assert_eq!(
        std::io::Error::from(error).kind(),
        std::io::ErrorKind::InvalidData
    );
}

fn assert_wal_snapshot_rejected_atomically(snapshot: EntitySnapshot) {
    let tmp = tempfile::tempdir().unwrap();
    let decision = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
        RegionPhase(1),
        1,
        1,
        vec![snapshot],
        Vec::new(),
    )
    .unwrap();
    write_framed_wal(tmp.path(), [framed_wal_group(&[decision])]);
    let (journal, pending) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    drop(journal);
    let registry = SessionRegistry::new();

    let recovery = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let replayed = replay_regional_commit_decisions(
            PersistedEntityCheckpoint::new(0, Vec::<PersistedEntityRecord>::new()),
            &pending,
        )?;
        Ok::<_, RegionalDecisionReplayError>(registry.restore_persisted_entities(replayed))
    }));

    let error = recovery
        .expect("invalid WAL recovery must not panic")
        .expect_err("invalid WAL snapshot must fail before ECS restore");
    assert!(matches!(
        error,
        RegionalDecisionReplayError::InvalidSnapshot
    ));
    assert_eq!(
        std::io::Error::from(error).kind(),
        std::io::ErrorKind::InvalidData
    );
    assert!(registry.persisted_entity_records().is_empty());
}

#[test]
fn wal_replay_rejects_unknown_entity_type_without_partial_restore_or_panic() {
    let mut unknown = snapshot(73, EntityLifecycle::Alive);
    unknown.type_id = 999;
    unknown.type_name = "minecraft:not_a_real_entity".into();

    assert_wal_snapshot_rejected_atomically(unknown);
}

#[test]
fn wal_replay_rejects_invalid_living_state_without_partial_restore_or_panic() {
    let mut invalid = snapshot(74, EntityLifecycle::Alive);
    invalid.health = 0.0;

    assert_wal_snapshot_rejected_atomically(invalid);
}

#[test]
fn wal_replay_rejects_invalid_active_effects_without_partial_restore_or_panic() {
    use mc_entity::effects_26_1_2::{ActiveEffectsSnapshot, EffectId};

    let mut invalid = snapshot(75, EntityLifecycle::Alive);
    invalid.retained.active_effects = Some(mc_entity::EntityActiveEffectsState {
        effects: ActiveEffectsSnapshot::default(),
        action_order: vec![EffectId::new(10)],
    });

    assert_wal_snapshot_rejected_atomically(invalid);
}

#[test]
fn valid_wal_replay_remains_idempotent() {
    let snapshot = snapshot(76, EntityLifecycle::Alive);
    let decision = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
        RegionPhase(1),
        1,
        1,
        vec![snapshot.clone()],
        Vec::new(),
    )
    .unwrap();
    let initial =
        PersistedEntityCheckpoint::new_at_owner_sequence(0, 0, Vec::<PersistedEntityRecord>::new());

    let once = replay_regional_commit_decisions(initial, std::slice::from_ref(&decision)).unwrap();
    let twice =
        replay_regional_commit_decisions(once.clone(), std::slice::from_ref(&decision)).unwrap();

    assert_eq!(once.lifecycle_clock, twice.lifecycle_clock);
    assert_eq!(
        once.regional_sequence_watermark,
        twice.regional_sequence_watermark
    );
    assert_eq!(once.records.len(), 1);
    assert_eq!(once.records[0].snapshot, snapshot);
    assert_eq!(once.records[0].snapshot, twice.records[0].snapshot);
}

#[test]
fn regional_decision_journal_recovers_grouped_crash_prefix_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let path = journal_path(tmp.path());
    let decisions = [decision(1, 1), decision(2, 2)];
    let (mut journal, _) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    journal.record_commits(&decisions).unwrap();
    drop(journal);

    let expected = framed_wal_group(&decisions);
    let mut bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[REGIONAL_DECISION_JOURNAL_HEADER.len()..], expected);
    bytes.truncate(bytes.len() - 1);
    std::fs::write(&path, bytes).unwrap();

    let (journal, pending) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    assert!(pending.is_empty());
    drop(journal);
    assert_eq!(
        std::fs::read(path).unwrap(),
        REGIONAL_DECISION_JOURNAL_HEADER
    );
}

#[test]
fn regional_decision_journal_limits_accept_exact_max_and_reject_max_plus_one() {
    validate_regional_journal_file_len(MAX_REGIONAL_DECISION_JOURNAL_BYTES).unwrap();
    assert!(validate_regional_journal_file_len(MAX_REGIONAL_DECISION_JOURNAL_BYTES + 1).is_err());

    validate_regional_journal_frame_payload_len(MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES).unwrap();
    assert!(
        validate_regional_journal_frame_payload_len(MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES + 1)
            .is_err()
    );

    validate_regional_decision_group_shape(
        MAX_REGIONAL_DECISIONS_PER_FRAME,
        MAX_REGIONAL_ENTITY_MUTATIONS_PER_FRAME,
    )
    .unwrap();
    assert!(
        validate_regional_decision_group_shape(MAX_REGIONAL_DECISIONS_PER_FRAME + 1, 0).is_err()
    );
    assert!(
        validate_regional_decision_group_shape(1, MAX_REGIONAL_ENTITY_MUTATIONS_PER_FRAME + 1)
            .is_err()
    );
}

#[test]
fn regional_decision_journal_rejects_oversized_sparse_file_before_read_allocation() {
    let tmp = tempfile::tempdir().unwrap();
    let path = journal_path(tmp.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = File::create(&path).unwrap();
    file.set_len(MAX_REGIONAL_DECISION_JOURNAL_BYTES + 1)
        .unwrap();
    drop(file);

    assert!(matches!(
        open_journal_error(tmp.path()),
        RegionalDecisionJournalOpenError::Framing {
            reason: "journal file exceeds operational limit",
            ..
        }
    ));
}

#[test]
fn regional_decision_journal_rejects_oversized_frame_metadata_before_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let oversized = u32::try_from(MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES + 1).unwrap();
    let mut frame = Vec::from(oversized.to_be_bytes());
    frame.extend_from_slice(&0_u32.to_be_bytes());
    write_framed_wal(tmp.path(), [frame]);

    assert!(matches!(
        open_journal_error(tmp.path()),
        RegionalDecisionJournalOpenError::Framing {
            reason: "record payload exceeds operational limit",
            ..
        }
    ));
}
