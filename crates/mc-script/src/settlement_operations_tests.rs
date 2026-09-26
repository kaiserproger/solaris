use crate::{
    MAX_SETTLEMENT_PLACEMENTS, MAX_VANILLA_VILLAGE_BUILDINGS, ScriptBuildingCertificate,
    ScriptBuildingCertificateRequest, ScriptBuildingInteractionPoint, ScriptChunkAvailability,
    ScriptDtoError, ScriptResidentSiteReservation, ScriptSettlementBuilding,
    ScriptSettlementOperation, ScriptSettlementPoi, ScriptSettlementResult, ScriptSettlementSite,
    ScriptSettlementSitePage, ScriptSitePoiKind, ScriptSitePoiState, ScriptSiteProvenance,
    ScriptSiteVariant, ScriptStructureMaterial, ScriptStructureReceipt, ScriptStructureSnapshot,
    ScriptStructureStagePlan, ScriptStructureState, ScriptSurveyBounds, ScriptSurveyPurpose,
    ScriptSurveySnapshot, ScriptWarehouseBinding, ScriptWarehouseSource, warehouse_handle,
};

const GENERATION_ID_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const GENERATION_ID_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn building(blueprint_id: &str) -> ScriptSettlementBuilding {
    ScriptSettlementBuilding::new(blueprint_id.to_owned(), [0, 64, 0], 90)
}

fn poi(poi_id: &str) -> ScriptSettlementPoi {
    ScriptSettlementPoi {
        poi_id: poi_id.to_owned(),
        kind: ScriptSitePoiKind::Home,
        at: [1, 64, 1],
        capacity: 2,
        state: ScriptSitePoiState::Free,
    }
}

fn site(site_id: &str) -> ScriptSettlementSite {
    ScriptSettlementSite {
        site_id: site_id.to_owned(),
        provenance: ScriptSiteProvenance::Authored,
        variant: ScriptSiteVariant::Village,
        revision: 7,
        contents_known: true,
        footprint_origin: [0, 64, 0],
        footprint_size: [32, 16, 32],
        buildings: vec![building("settlement:house")],
        pois: vec![poi("home-0")],
        inhabitant_generation_ids: vec![GENERATION_ID_A.to_owned()],
    }
}

fn material(resource: &str, quantity: u64) -> ScriptStructureMaterial {
    ScriptStructureMaterial::new(resource.to_owned(), quantity)
}

fn stage_plan(resources: usize) -> ScriptStructureStagePlan {
    ScriptStructureStagePlan {
        stage: "foundation".to_owned(),
        block_count: 16,
        work_units: 32,
        materials: (0..resources)
            .map(|index| material(&format!("solaris:r{index}"), 1))
            .collect(),
    }
}

fn structure_snapshot() -> ScriptStructureSnapshot {
    ScriptStructureSnapshot {
        structure_id: "structure-1".to_owned(),
        blueprint_id: "settlement:house".to_owned(),
        site_id: "site-1".to_owned(),
        state: ScriptStructureState::Running,
        revision: 4,
        origin: [16, 64, 16],
        rotation: 180,
        reserved_footprint: [16, 8, 16],
        stages: vec![stage_plan(2)],
        current_stage_index: 0,
        completed_work_units: 4,
        resource_plan_hash: "a".repeat(64),
        reservation_ref: Some("reservation-1".to_owned()),
        watermark: 3,
        consumed: vec![material("solaris:r0", 4)],
        remaining: vec![material("solaris:r1", 6)],
        pause_reason: None,
    }
}

#[test]
fn rotation_outside_quarter_turns_is_rejected() {
    for rotation in [0, 90, 180, 270] {
        let building =
            ScriptSettlementBuilding::new("settlement:house".to_owned(), [0, 0, 0], rotation);
        assert!(
            building.validate().is_ok(),
            "rotation {rotation} must be valid"
        );
    }
    for rotation in [1, 45, 89, 270 + 1, 360, u16::MAX] {
        let building =
            ScriptSettlementBuilding::new("settlement:house".to_owned(), [0, 0, 0], rotation);
        assert!(
            matches!(building.validate(), Err(ScriptDtoError::InvalidBounds)),
            "rotation {rotation} must be rejected"
        );
    }
}

#[test]
fn survey_bounds_reject_non_positive_and_oversized_extents() {
    assert!(ScriptSurveyBounds::new([0, 0, 0], [127, 127, 127]).is_ok());
    assert!(ScriptSurveyBounds::new([10, 20, 30], [10, 20, 30]).is_ok());
    // 129 columns on the x axis is one past the surveyed maximum.
    assert!(matches!(
        ScriptSurveyBounds::new([0, 0, 0], [128, 127, 127]),
        Err(ScriptDtoError::InvalidBounds)
    ));
    // 129 columns on the z axis.
    assert!(matches!(
        ScriptSurveyBounds::new([-64, 0, -64], [-64, 0, 64]),
        Err(ScriptDtoError::InvalidBounds)
    ));
    // Non-positive extent (min past max).
    assert!(matches!(
        ScriptSurveyBounds::new([0, 0, 0], [-1, 0, 0]),
        Err(ScriptDtoError::InvalidBounds)
    ));
    // Overflowing extents must fail closed rather than wrap.
    assert!(matches!(
        ScriptSurveyBounds::new([i32::MIN, 0, 0], [i32::MAX, 0, 0]),
        Err(ScriptDtoError::InvalidBounds)
    ));
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [127, 63, 15]).expect("valid bounds");
    assert_eq!(bounds.columns(), 128 * 16);
    assert_eq!(bounds.min(), [0, 0, 0]);
    assert_eq!(bounds.max(), [127, 63, 15]);
}

/// A survey snapshot carries aggregate counts, never one record per column, so
/// its size is independent of the tile: even a full 128x128 tile validates while
/// its aggregates may not exceed the tile it surveyed.
#[test]
fn survey_snapshot_is_bounded_by_aggregate_counts() {
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [127, 31, 127]).expect("valid bounds");
    let full = ScriptSurveySnapshot::new(
        "minecraft:overworld".to_owned(),
        bounds,
        1,
        1,
        ScriptChunkAvailability::Loaded,
        "survey-1".to_owned(),
        16_000,
        384,
        false,
        0,
        vec!["minecraft:plains".to_owned()],
        Vec::new(),
    );
    assert!(full.validate().is_ok());

    let over = ScriptSurveySnapshot {
        usable_plots: 128 * 128 + 1,
        ..full.clone()
    };
    assert!(matches!(
        over.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
}

#[test]
fn site_page_rejects_more_than_the_page_limit() {
    let full = ScriptSettlementSitePage::new(
        (0..64)
            .map(|index| site(&format!("site-{index}")))
            .collect(),
        None,
    );
    assert!(full.validate().is_ok());

    let overflow = ScriptSettlementSitePage::new(
        (0..65)
            .map(|index| site(&format!("site-{index}")))
            .collect(),
        None,
    );
    assert!(matches!(
        overflow.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));

    // Duplicate site ids inside one page are rejected.
    let duplicated = ScriptSettlementSitePage::new(vec![site("site-1"), site("site-1")], None);
    assert!(matches!(
        duplicated.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
}

#[test]
fn advance_structure_rejects_zero_or_excessive_work_units() {
    let advance = |work_units: u64| ScriptSettlementOperation::AdvanceStructure {
        operation_id: "advance-1".to_owned(),
        structure_id: "structure-1".to_owned(),
        stage: "foundation".to_owned(),
        reservation_ref: "reservation-1".to_owned(),
        expected_revision: 4,
        work_units,
    };
    assert!(advance(1).validate().is_ok());
    assert!(advance(512).validate().is_ok());
    assert!(matches!(
        advance(0).validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
    assert!(matches!(
        advance(513).validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));

    let mut snapshot = structure_snapshot();
    snapshot.current_stage_index = 1;
    assert!(matches!(
        snapshot.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
    snapshot.current_stage_index = 0;
    snapshot.completed_work_units = 33;
    assert!(matches!(
        snapshot.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
    snapshot.current_stage_index = 1;
    snapshot.completed_work_units = 0;
    snapshot.state = ScriptStructureState::Committed;
    assert!(snapshot.validate().is_ok());
    snapshot.completed_work_units = 1;
    assert!(matches!(
        snapshot.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));

    let zero_plan = ScriptStructureStagePlan {
        work_units: 0,
        ..stage_plan(1)
    };
    assert!(matches!(
        zero_plan.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
}

#[test]
fn materials_reject_more_than_sixteen_distinct_resources() {
    assert!(stage_plan(16).validate().is_ok());
    assert!(matches!(
        stage_plan(17).validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));

    // Repeating a resource is a duplicate, not a second resource type.
    let duplicate = ScriptStructureStagePlan {
        materials: vec![material("solaris:r0", 1), material("solaris:r0", 2)],
        ..stage_plan(0)
    };
    assert!(matches!(
        duplicate.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
}

#[test]
fn operation_id_is_exposed_only_for_idempotent_mutations() {
    assert_eq!(
        ScriptSettlementOperation::ReserveResidentSite {
            operation_id: "reserve-1".to_owned(),
            site_id: "site-1".to_owned(),
            poi_id: "home-0".to_owned(),
            expected_site_revision: 1,
        }
        .operation_id(),
        Some("reserve-1")
    );
    assert_eq!(
        ScriptSettlementOperation::ReleaseResidentSite {
            operation_id: "release-1".to_owned(),
            spawn_site_token: "spawn-1".to_owned(),
        }
        .operation_id(),
        Some("release-1")
    );
    assert_eq!(
        ScriptSettlementOperation::PrepareStructure {
            operation_id: "prepare-1".to_owned(),
            site_id: "site-1".to_owned(),
            blueprint_id: "settlement:house".to_owned(),
            anchor: [0, 64, 0],
            rotation: 0,
            survey_token: "survey-1".to_owned(),
            expected_site_revision: 1,
            expected_blueprint_hash: None,
        }
        .operation_id(),
        Some("prepare-1")
    );
    assert_eq!(
        ScriptSettlementOperation::PauseStructure {
            operation_id: "pause-1".to_owned(),
            structure_id: "structure-1".to_owned(),
            expected_revision: 1,
        }
        .operation_id(),
        Some("pause-1")
    );
    assert_eq!(
        ScriptSettlementOperation::ResumeStructure {
            operation_id: "resume-1".to_owned(),
            structure_id: "structure-1".to_owned(),
            expected_revision: 1,
        }
        .operation_id(),
        Some("resume-1")
    );
    assert_eq!(
        ScriptSettlementOperation::CancelStructure {
            operation_id: "cancel-1".to_owned(),
            structure_id: "structure-1".to_owned(),
            expected_revision: 1,
        }
        .operation_id(),
        Some("cancel-1")
    );
    assert_eq!(
        ScriptSettlementOperation::ListSites {
            cursor: None,
            limit: 32,
        }
        .operation_id(),
        None
    );
    assert_eq!(
        ScriptSettlementOperation::QuerySite {
            site_id: "site-1".to_owned(),
            cursor: None,
            limit: 32,
        }
        .operation_id(),
        None
    );
    assert_eq!(
        ScriptSettlementOperation::Survey {
            dimension: "minecraft:overworld".to_owned(),
            bounds: ScriptSurveyBounds::new([0, 0, 0], [15, 15, 15]).expect("valid bounds"),
            purpose: ScriptSurveyPurpose::Settlement,
        }
        .operation_id(),
        None
    );
    assert_eq!(
        ScriptSettlementOperation::Status {
            structure_id: "structure-1".to_owned(),
        }
        .operation_id(),
        None
    );

    let empty = ScriptSettlementOperation::PauseStructure {
        operation_id: String::new(),
        structure_id: "structure-1".to_owned(),
        expected_revision: 1,
    };
    assert!(matches!(
        empty.validate(),
        Err(ScriptDtoError::EmptyValue { .. })
    ));
}

#[test]
fn canonicalize_orders_site_payloads_regardless_of_input_order() {
    let mut left = site("site-1");
    left.buildings = vec![
        building("settlement:house"),
        building("settlement:smith"),
        building("settlement:house"),
    ];
    left.pois = vec![poi("work-0"), poi("home-0"), poi("guard-0")];
    left.inhabitant_generation_ids = vec![GENERATION_ID_B.to_owned(), GENERATION_ID_A.to_owned()];

    let mut right = site("site-1");
    right.buildings = vec![
        building("settlement:smith"),
        building("settlement:house"),
        building("settlement:house"),
    ];
    right.pois = vec![poi("guard-0"), poi("home-0"), poi("work-0")];
    right.inhabitant_generation_ids = vec![GENERATION_ID_A.to_owned(), GENERATION_ID_B.to_owned()];

    assert_ne!(left, right);
    left.canonicalize();
    right.canonicalize();
    assert_eq!(left, right);
    assert!(left.validate().is_ok(), "{:?}", left.validate());

    let mut page_left = ScriptSettlementSitePage::new(vec![site("site-2"), site("site-1")], None);
    let mut page_right = ScriptSettlementSitePage::new(vec![site("site-1"), site("site-2")], None);
    page_left.canonicalize();
    page_right.canonicalize();
    assert_eq!(page_left, page_right);
}

#[test]
fn serde_round_trips_one_full_site_and_one_full_structure() {
    let site = site("site-1");
    let encoded = serde_json::to_string(&site).expect("site serializes");
    let decoded: ScriptSettlementSite = serde_json::from_str(&encoded).expect("site deserializes");
    assert_eq!(decoded, site);
    assert!(decoded.validate().is_ok());

    let snapshot = structure_snapshot();
    let encoded = serde_json::to_string(&snapshot).expect("structure serializes");
    let decoded: ScriptStructureSnapshot =
        serde_json::from_str(&encoded).expect("structure deserializes");
    assert_eq!(decoded, snapshot);
    assert!(decoded.validate().is_ok());

    let survey = ScriptSurveySnapshot {
        dimension: "minecraft:overworld".to_owned(),
        bounds: ScriptSurveyBounds::new([0, 0, 0], [1, 1, 1]).expect("valid bounds"),
        revision: 2,
        world_revision: 2,
        chunk_availability: ScriptChunkAvailability::Loaded,
        survey_token: "survey-1".to_owned(),
        usable_plots: 2,
        water_columns: 0,
        claimed: false,
        existing_structures: 0,
        biome_tags: vec!["minecraft:plains".to_owned()],
        resource_tags: vec!["fertile".to_owned()],
    };
    let encoded = serde_json::to_string(&survey).expect("survey serializes");
    let decoded: ScriptSurveySnapshot =
        serde_json::from_str(&encoded).expect("survey deserializes");
    assert_eq!(decoded, survey);
    assert!(decoded.validate().is_ok());

    let reservation = ScriptResidentSiteReservation {
        site_id: "site-1".to_owned(),
        poi_id: "home-0".to_owned(),
        spawn_site_token: "spawn-1".to_owned(),
        revision: 1,
    };
    let receipt = ScriptStructureReceipt {
        structure_id: "structure-1".to_owned(),
        stage: "foundation".to_owned(),
        sequence: 1,
        block_count: 16,
        work_units: 16,
        consumed: vec![material("solaris:r0", 2)],
        revision: 1,
    };
    assert!(
        ScriptSettlementResult::Sites {
            page: Box::new(ScriptSettlementSitePage::new(vec![site.clone()], None)),
        }
        .validate()
        .is_ok()
    );
    assert!(
        ScriptSettlementResult::Site {
            site: Box::new(site),
        }
        .validate()
        .is_ok()
    );
    assert!(
        ScriptSettlementResult::ResidentSite {
            reservation: Box::new(reservation),
        }
        .validate()
        .is_ok()
    );
    assert!(
        ScriptSettlementResult::Survey {
            survey: Box::new(survey),
        }
        .validate()
        .is_ok()
    );
    assert!(
        ScriptSettlementResult::Structure {
            structure: Box::new(snapshot),
        }
        .validate()
        .is_ok()
    );
    assert!(
        ScriptSettlementResult::Receipt {
            receipt: Box::new(receipt),
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn warehouse_binding_and_handle_are_bounded_and_owner_scoped() {
    let handle = warehouse_handle("settlement", &"a".repeat(64), 3).expect("handle");
    assert_eq!(handle, format!("warehouse:settlement:{}:3", "a".repeat(64)));
    let binding = ScriptWarehouseBinding::new(
        handle,
        ScriptWarehouseSource::Authored {
            structure_id: "a".repeat(64),
            container_id: 3,
        },
        5,
    );
    assert!(binding.validate().is_ok());
    assert!(
        ScriptSettlementResult::Warehouse {
            binding: Box::new(binding),
        }
        .validate()
        .is_ok()
    );

    let legacy = serde_json::json!({
        "handle": "warehouse:settlement:structure-1:3",
        "structure_id": "structure-1",
        "container_id": 3,
        "revision": 5,
    });
    assert_eq!(
        serde_json::from_value::<ScriptWarehouseBinding>(legacy).expect("legacy binding decodes"),
        ScriptWarehouseBinding::new(
            "warehouse:settlement:structure-1:3".to_owned(),
            ScriptWarehouseSource::Authored {
                structure_id: "structure-1".to_owned(),
                container_id: 3,
            },
            5,
        )
    );

    // A handle that cannot fit the opaque 128-byte bound is refused at mint.
    let long = "p".repeat(crate::MAX_PLUGIN_ID_BYTES);
    let minted = format!("warehouse:{long}:{}:0", "a".repeat(64));
    assert_eq!(
        warehouse_handle(&long, &"a".repeat(64), 0).unwrap_err(),
        ScriptDtoError::ValueTooLong {
            field: "warehouse handle",
            max_bytes: crate::MAX_WAREHOUSE_HANDLE_BYTES,
            actual_bytes: minted.len(),
        }
    );

    let operation = ScriptSettlementOperation::BindWarehouse {
        operation_id: "bind-1".to_owned(),
        structure_id: "a".repeat(64),
        container_id: 2,
    };
    assert_eq!(operation.operation_id(), Some("bind-1"));
    assert!(operation.validate().is_ok());
    let encoded = serde_json::to_string(&operation).expect("bind serializes");
    assert_eq!(
        serde_json::from_str::<ScriptSettlementOperation>(&encoded).expect("bind deserializes"),
        operation
    );

    let village = ScriptSettlementOperation::BindVillageWarehouse {
        operation_id: "bind-village-1".to_owned(),
        site_id: "village_3_5_0123456789abcdef".to_owned(),
        container_id: 2,
    };
    assert_eq!(village.operation_id(), Some("bind-village-1"));
    assert!(village.validate().is_ok());

    let manual = ScriptWarehouseBinding::new(
        "warehouse:settlement:manual:1:2:3".to_owned(),
        ScriptWarehouseSource::Manual {
            position: [1, 2, 3],
        },
        5,
    );
    assert!(manual.validate().is_ok());
    let far = ScriptWarehouseBinding::new(
        "warehouse:settlement:manual:far".to_owned(),
        ScriptWarehouseSource::Manual {
            position: [30_000_001, 0, 0],
        },
        5,
    );
    assert!(matches!(far.validate(), Err(ScriptDtoError::InvalidBounds)));

    let bind_manual = ScriptSettlementOperation::BindManualWarehouse {
        operation_id: "bind-manual-1".to_owned(),
        survey_token: "t".repeat(64),
        position: [1, 2, 3],
    };
    assert_eq!(bind_manual.operation_id(), Some("bind-manual-1"));
    assert!(bind_manual.validate().is_ok());
    let encoded = serde_json::to_string(&bind_manual).expect("manual bind serializes");
    assert_eq!(
        serde_json::from_str::<ScriptSettlementOperation>(&encoded)
            .expect("manual bind deserializes"),
        bind_manual
    );
    let outside = ScriptSettlementOperation::BindManualWarehouse {
        operation_id: "bind-manual-2".to_owned(),
        survey_token: "t".repeat(64),
        position: [30_000_001, 0, 0],
    };
    assert!(matches!(
        outside.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
}
#[test]
fn serde_rejects_unknown_keys() {
    let operation = serde_json::json!({
        "kind": "status",
        "structure_id": "structure-1",
        "unexpected": true,
    });
    assert!(serde_json::from_str::<ScriptSettlementOperation>(&operation.to_string()).is_err());

    let mut value = serde_json::to_value(site("site-1")).expect("site value");
    value
        .as_object_mut()
        .expect("site object")
        .insert("unexpected".to_owned(), serde_json::Value::Bool(true));
    assert!(serde_json::from_value::<ScriptSettlementSite>(value).is_err());
}

/// A settlement site is territory (128/192/256 by variant) and must not be
/// squeezed into the per-building blueprint bound; the two bounds stay distinct.
#[test]
fn site_territory_footprint_is_accepted_above_the_blueprint_bound() {
    let mut town = site("site-town");
    town.footprint_size = [256, 40, 256];
    assert!(town.validate().is_ok());

    town.footprint_size = [257, 40, 256];
    assert!(matches!(
        town.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));

    // A building blueprint stays bounded at 64 per axis.
    let mut house = structure_snapshot();
    house.reserved_footprint = [64, 9, 64];
    assert!(house.validate().is_ok());
    house.reserved_footprint = [65, 9, 64];
    assert!(matches!(
        house.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
}

/// Vanilla villages describe all placed templates, so their independently bounded
/// descriptor capacity must not weaken the authored catalog-site contract.
#[test]
fn vanilla_village_buildings_use_a_separate_descriptor_bound() {
    let mut village = site("site-village");
    village.provenance = ScriptSiteProvenance::VanillaVillage;
    village.inhabitant_generation_ids.clear();
    village.buildings =
        vec![building("minecraft:village_house"); MAX_SETTLEMENT_PLACEMENTS.saturating_add(1)];
    assert!(village.validate().is_ok());

    village.buildings =
        vec![building("minecraft:village_house"); MAX_VANILLA_VILLAGE_BUILDINGS.saturating_add(1)];
    assert!(matches!(
        village.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));

    village.provenance = ScriptSiteProvenance::Authored;
    assert!(matches!(
        village.validate(),
        Err(ScriptDtoError::InvalidBounds)
    ));
}

fn certificate_footprint() -> ScriptSurveyBounds {
    ScriptSurveyBounds::new([0, 60, 0], [15, 67, 15]).expect("valid footprint")
}

fn certificate_points() -> Vec<ScriptBuildingInteractionPoint> {
    vec![
        ScriptBuildingInteractionPoint::Entrance { at: [1, 61, 1] },
        ScriptBuildingInteractionPoint::Poi {
            poi_kind: ScriptSitePoiKind::Home,
            at: [2, 61, 2],
            capacity: 2,
        },
        ScriptBuildingInteractionPoint::Workstation {
            at: [3, 61, 3],
            capability: "solaris:smithy".to_owned(),
            workplaces: 2,
        },
        ScriptBuildingInteractionPoint::Storage { at: [4, 61, 4] },
    ]
}

fn certificate_request() -> ScriptBuildingCertificateRequest {
    ScriptBuildingCertificateRequest::new(
        certificate_footprint(),
        "solaris:smithy".to_owned(),
        certificate_points(),
        "t".repeat(64),
        7,
    )
}

/// The certificate request carries the §4.3 fields, round-trips, and refuses
/// malformed fences: bad purpose, missing/foreign survey token, points outside
/// the footprint, duplicates, empty lists, and zero capacity/workplaces.
#[test]
fn building_certificate_request_bounds_and_round_trip() {
    let mut request = certificate_request();
    request.canonicalize();
    assert!(request.validate().is_ok());
    let encoded = serde_json::to_string(&request).expect("request serializes");
    assert_eq!(
        serde_json::from_str::<ScriptBuildingCertificateRequest>(&encoded)
            .expect("request deserializes"),
        request
    );

    let mut bad_purpose = certificate_request();
    bad_purpose.purpose = "not a resource id".to_owned();
    assert!(bad_purpose.validate().is_err());

    let mut no_token = certificate_request();
    no_token.survey_token.clear();
    assert!(no_token.validate().is_err());

    let mut outside = certificate_request();
    outside
        .interaction_points
        .push(ScriptBuildingInteractionPoint::Entrance { at: [100, 64, 100] });
    assert!(outside.validate().is_err());

    let mut duplicate = certificate_request();
    duplicate
        .interaction_points
        .push(ScriptBuildingInteractionPoint::Entrance { at: [1, 61, 1] });
    assert!(duplicate.validate().is_err());

    let mut empty = certificate_request();
    empty.interaction_points.clear();
    assert!(empty.validate().is_err());

    let mut zero_capacity = certificate_request();
    zero_capacity.interaction_points = vec![ScriptBuildingInteractionPoint::Poi {
        poi_kind: ScriptSitePoiKind::Home,
        at: [1, 61, 1],
        capacity: 0,
    }];
    assert!(zero_capacity.validate().is_err());

    let mut zero_workplaces = certificate_request();
    zero_workplaces.interaction_points = vec![ScriptBuildingInteractionPoint::Workstation {
        at: [1, 61, 1],
        capability: "solaris:smithy".to_owned(),
        workplaces: 0,
    }];
    assert!(zero_workplaces.validate().is_err());

    let mut bad_capability = certificate_request();
    bad_capability.interaction_points = vec![ScriptBuildingInteractionPoint::Workstation {
        at: [1, 61, 1],
        capability: "not a capability".to_owned(),
        workplaces: 1,
    }];
    assert!(bad_capability.validate().is_err());
}

/// The certificate mirrors the accepted request with its own opaque handle and
/// revisions, round-trips, and refuses a missing handle or moved points.
#[test]
fn building_certificate_bounds_and_round_trip() {
    let request = certificate_request();
    let mut certificate = ScriptBuildingCertificate::new(
        "certificate-1".to_owned(),
        request.footprint,
        request.purpose.clone(),
        request.interaction_points.clone(),
        7,
        3,
    );
    certificate.canonicalize();
    assert!(certificate.validate().is_ok());
    let encoded = serde_json::to_string(&certificate).expect("certificate serializes");
    assert_eq!(
        serde_json::from_str::<ScriptBuildingCertificate>(&encoded)
            .expect("certificate deserializes"),
        certificate
    );

    let mut no_handle = certificate.clone();
    no_handle.certificate_id.clear();
    assert!(no_handle.validate().is_err());

    let mut outside = certificate.clone();
    outside
        .interaction_points
        .push(ScriptBuildingInteractionPoint::Storage { at: [100, 64, 100] });
    assert!(outside.validate().is_err());
}
