use crate::{
    MAX_RESIDENT_QUERY_HANDLES, ScriptDtoError, ScriptOperation, ScriptOperationPayload,
    ScriptOperationRequest, ScriptResidentKind, ScriptResidentLifecycle, ScriptResidentLoadedState,
    ScriptResidentOperation, ScriptResidentPois, ScriptResidentProfile, ScriptResidentResult,
    ScriptResidentSnapshot, resident_entity_uuid, resident_generation_id,
    resident_generation_id_for_poi, resident_handle_for_entity, resident_handle_for_generation,
    resident_spawn_site_token, validate_generation_id,
};

const UUID: &str = "12345678-1234-5678-1234-567812345678";

fn claim(operation_id: &str) -> ScriptResidentOperation {
    ScriptResidentOperation::Claim {
        operation_id: operation_id.to_owned(),
        actor_id: 7,
        entity_uuid: UUID.to_owned(),
        expected_entity_revision: 0,
    }
}

#[test]
fn generation_ids_and_handles_are_deterministic_and_domain_separated() {
    let first = resident_generation_id("world-1", "site-7", 3).unwrap();
    let same = resident_generation_id("world-1", "site-7", 3).unwrap();
    assert_eq!(first, same);
    validate_generation_id(&first).unwrap();
    assert_eq!(first.len(), 64);

    assert_ne!(
        first,
        resident_generation_id("world-1", "site-7", 4).unwrap()
    );
    assert_ne!(
        first,
        resident_generation_id("world-1", "site-8", 3).unwrap()
    );
    assert_ne!(
        first,
        resident_generation_id("world-2", "site-7", 3).unwrap()
    );

    let handle = resident_handle_for_generation("settlement", &first).unwrap();
    assert_eq!(
        handle,
        resident_handle_for_generation("settlement", &first).unwrap()
    );
    assert_ne!(
        handle,
        resident_handle_for_generation("other-plugin", &first).unwrap()
    );
    assert_ne!(
        handle,
        resident_handle_for_entity("settlement", UUID).unwrap()
    );
    // The same generation always materialises the same entity UUID.
    assert_eq!(
        resident_entity_uuid(&first).unwrap(),
        resident_entity_uuid(&first).unwrap()
    );
    assert_ne!(
        resident_entity_uuid(&first).unwrap(),
        resident_entity_uuid(&resident_generation_id("world-1", "site-7", 4).unwrap()).unwrap()
    );
    let entity_uuid = resident_entity_uuid(&first).unwrap();
    assert_eq!(entity_uuid[6] >> 4, 8);
    assert_eq!(entity_uuid[8] >> 6, 0b10);

    let token = resident_spawn_site_token("settlement", &first).unwrap();
    assert_eq!(
        token,
        resident_spawn_site_token("settlement", &first).unwrap()
    );
    assert_ne!(
        token,
        resident_spawn_site_token("other-plugin", &first).unwrap()
    );

    assert!(matches!(
        resident_generation_id("", "site-7", 0),
        Err(ScriptDtoError::EmptyValue { .. })
    ));
    assert!(matches!(
        resident_generation_id("world-1", "site-7", 0)
            .map(|id| validate_generation_id(&id.to_uppercase())),
        Ok(Err(ScriptDtoError::InvalidId { .. }))
    ));
}

#[test]
fn poi_generation_ids_are_stable_and_not_descriptor_slots() {
    let first = resident_generation_id_for_poi("world-1", "village-7", "village_poi_3_64_5")
        .expect("valid physical POI");
    assert_eq!(
        first,
        resident_generation_id_for_poi("world-1", "village-7", "village_poi_3_64_5").unwrap()
    );
    assert_ne!(
        first,
        resident_generation_id_for_poi("world-1", "village-7", "village_poi_4_64_5").unwrap()
    );
    assert_ne!(
        first,
        resident_generation_id("world-1", "village-7", 0).unwrap()
    );
    validate_generation_id(&first).unwrap();
}

#[test]
fn generation_ids_are_independent_of_world_directory_length() {
    // A server world directory can be deeper than the 128-byte plugin-identity
    // bound; the harness default run root already produced a 143-character
    // world path. The generation id is a fixed-width hash, so it must accept
    // any directory depth while staying deterministic and distinct.
    let nested = "nested-segment/".repeat(16);
    let deep = std::env::temp_dir().join(format!("{nested}world"));
    let other = std::env::temp_dir().join(format!("{nested}other-world"));
    let deep = deep.to_string_lossy();
    let other = other.to_string_lossy();
    assert!(deep.len() > 160, "{} bytes", deep.len());

    let first = resident_generation_id(&deep, "site-7", 0).expect("long world directory");
    validate_generation_id(&first).unwrap();
    assert_eq!(first.len(), 64);
    assert_eq!(first, resident_generation_id(&deep, "site-7", 0).unwrap());
    assert_ne!(first, resident_generation_id(&other, "site-7", 0).unwrap());

    // A short directory keeps its persisted generation id: only the length
    // check was removed, never the derivation itself.
    assert_eq!(
        resident_generation_id("world-1", "site-7", 3).unwrap(),
        "853f1a50d8acc3c70030dda66d59ecce16ced9669ad5cac176c5f0ee49616e2b"
    );
}

#[test]
fn query_handles_are_bounded_sorted_and_deduplicated() {
    let handles = vec!["b2".to_owned(), "a1".to_owned(), "a1".to_owned()];
    let request = ScriptOperationRequest::try_new(
        "query",
        ScriptOperation::Resident {
            operation: ScriptResidentOperation::Query {
                handles,
                cursor: None,
            },
        },
    )
    .unwrap();
    let ScriptOperation::Resident {
        operation: ScriptResidentOperation::Query { handles, .. },
    } = request.operation()
    else {
        panic!("resident query envelope");
    };
    assert_eq!(handles, &["a1".to_owned(), "b2".to_owned()]);

    let oversized = (0..=MAX_RESIDENT_QUERY_HANDLES)
        .map(|index| format!("h{index:02}"))
        .collect::<Vec<_>>();
    assert!(matches!(
        ScriptOperationRequest::try_new(
            "query",
            ScriptOperation::Resident {
                operation: ScriptResidentOperation::Query {
                    handles: oversized,
                    cursor: None,
                },
            },
        ),
        Err(ScriptDtoError::TooManyEntries {
            field: "resident query handles",
            max: MAX_RESIDENT_QUERY_HANDLES,
        })
    ));
}

#[test]
fn resident_mutations_require_ids_fences_and_closed_profiles() {
    for rejected in [
        claim("Operation"),
        claim(&"x".repeat(crate::MAX_SCRIPT_ID_BYTES + 1)),
        ScriptResidentOperation::Claim {
            operation_id: "op".to_owned(),
            actor_id: 0,
            entity_uuid: UUID.to_owned(),
            expected_entity_revision: 0,
        },
        ScriptResidentOperation::Claim {
            operation_id: "op".to_owned(),
            actor_id: 7,
            entity_uuid: "not-a-uuid".to_owned(),
            expected_entity_revision: 0,
        },
        ScriptResidentOperation::Spawn {
            operation_id: "op".to_owned(),
            spawn_site_token: String::new(),
            profile: ScriptResidentProfile::new(ScriptResidentKind::Villager),
        },
        ScriptResidentOperation::Release {
            operation_id: "op".to_owned(),
            handle: "r1".to_owned(),
            expected_revision: crate::MAX_SCRIPT_WORLD_TIME + 1,
        },
        ScriptResidentOperation::SetPois {
            operation_id: "op".to_owned(),
            handle: "r1".to_owned(),
            home_poi: Some(String::new()),
            work_poi: None,
            meeting_poi: None,
            expected_revision: 0,
        },
    ] {
        assert!(
            matches!(
                ScriptOperationRequest::try_new(
                    "request",
                    ScriptOperation::Resident {
                        operation: rejected.clone(),
                    },
                ),
                Err(ScriptDtoError::InvalidId { .. }
                    | ScriptDtoError::InvalidBounds
                    | ScriptDtoError::EmptyValue { .. }
                    | ScriptDtoError::ValueTooLong { .. })
            ),
            "accepted {rejected:?}"
        );
    }
}

#[test]
fn resident_snapshots_bind_lifecycle_to_loaded_state() {
    let record = ScriptResidentSnapshot::new(
        "a1".to_owned(),
        UUID.to_owned(),
        ScriptResidentLifecycle::AliveLoaded,
        9,
        None,
        ScriptResidentPois::default(),
        Some(ScriptResidentLoadedState::new(
            crate::ScriptPosition::try_new(1.0, 64.0, -2.0).unwrap(),
            20.0,
            Vec::new(),
        )),
    );
    record.validate().unwrap();
    let mut inconsistent = record.clone();
    inconsistent.loaded = None;
    assert!(matches!(
        inconsistent.validate(),
        Err(ScriptDtoError::InconsistentResult { .. })
    ));
    let unloaded = ScriptResidentSnapshot::new(
        "a1".to_owned(),
        UUID.to_owned(),
        ScriptResidentLifecycle::AliveUnloaded,
        9,
        None,
        ScriptResidentPois::default(),
        None,
    );
    unloaded.validate().unwrap();
    let mut nan = record;
    nan.loaded = Some(ScriptResidentLoadedState::new(
        crate::ScriptPosition::try_new(0.0, 0.0, 0.0).unwrap(),
        f32::NAN,
        Vec::new(),
    ));
    assert!(matches!(nan.validate(), Err(ScriptDtoError::InvalidBounds)));

    let outcome = crate::ScriptOperationOutcome::committed(
        4,
        ScriptOperationPayload::Resident {
            result: Box::new(ScriptResidentResult::Snapshot {
                resident: unloaded.clone(),
            }),
        },
    )
    .unwrap();
    assert_eq!(outcome.revision(), Some(4));
    assert!(matches!(
        outcome.payload(),
        ScriptOperationPayload::Resident { result }
            if matches!(&**result, ScriptResidentResult::Snapshot { .. })
    ));
    assert!(matches!(
        crate::ScriptOperationOutcome::committed(
            4,
            ScriptOperationPayload::Resident {
                result: Box::new(ScriptResidentResult::Page {
                    residents: vec![unloaded.clone(), unloaded],
                    cursor: None,
                }),
            },
        ),
        Err(ScriptDtoError::InconsistentResult { .. })
    ));
    assert!(!ScriptResidentLifecycle::Dead.occupies_living_capacity());
    assert!(ScriptResidentLifecycle::AliveUnloaded.occupies_living_capacity());
    assert_eq!(ScriptResidentLifecycle::Dead.as_str(), "dead");
}
