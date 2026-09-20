//! The settlement family of the contract, converted to and from the server's own
//! DTO.
//!
//! The generated `settlements` interface is a rename of the server's own
//! vocabulary and nothing more: `decode` builds exactly the
//! `mc_script::ScriptSettlementOperation` the guest described, `encode_result`
//! renames the server's typed result back, and neither invents a value, clamps one
//! to a bound, or substitutes a different site, structure or container for the one
//! a plugin named. Bounds are the DTO's own: `ScriptOperationRequest::try_new` is
//! what validates and canonicalizes a decoded operation before any owner sees it,
//! so this conversion only builds the DTO's own constructors - a survey bounds
//! whose extent is not 1..`MAX_SURVEY_BOUNDS_AXIS` is the guest's own malformed
//! answer and fails the batch there, exactly as a rotation that is not a quarter
//! turn does.
//!
//! A native variant this contract cannot express - the server's enums are
//! `non_exhaustive` on purpose - is never mapped onto a member that means
//! something else: `encode_result` answers `None` and the answer is dropped rather
//! than delivered with an invented shape.

use crate::bindings::solaris::plugin::settlements as wire;
use mc_script::{
    ScriptChunkAvailability, ScriptDtoError, ScriptResidentSiteReservation,
    ScriptSettlementBuilding, ScriptSettlementOperation, ScriptSettlementPoi,
    ScriptSettlementResult, ScriptSettlementSite, ScriptSettlementSitePage, ScriptSitePoiKind,
    ScriptSitePoiState, ScriptSiteProvenance, ScriptSiteVariant, ScriptStructureMaterial,
    ScriptStructureReceipt, ScriptStructureSnapshot, ScriptStructureStagePlan,
    ScriptStructureState, ScriptSurveyBounds, ScriptSurveyPurpose, ScriptSurveySnapshot,
    ScriptWarehouseBinding, ScriptWarehouseSource,
};

/// One settlement operation, as the server's own DTO.
///
/// The conversion is total where the contract is total: every id, coordinate,
/// cursor and revision the guest named is carried unchanged, and the one thing that
/// can refuse is the survey bounds constructor itself - the server's own
/// `ScriptSurveyBounds::new`, which refuses a non-positive or oversized extent.
/// Everything else the DTO bounds - the site, blueprint, structure, stage and token
/// lengths, the rotation, the page limit, the revision ceiling and the operation id
/// alphabet - is re-checked when the host hands the decoded operation to
/// `ScriptOperationRequest::try_new`.
pub(crate) fn decode(
    value: wire::SettlementOperation,
) -> Result<ScriptSettlementOperation, ScriptDtoError> {
    Ok(match value {
        wire::SettlementOperation::ListSites(list) => ScriptSettlementOperation::ListSites {
            cursor: list.cursor,
            limit: list.limit,
        },
        wire::SettlementOperation::QuerySite(query) => ScriptSettlementOperation::QuerySite {
            site_id: query.site_id,
            cursor: query.cursor,
            limit: query.limit,
        },
        wire::SettlementOperation::ReserveResidentSite(reserve) => {
            ScriptSettlementOperation::ReserveResidentSite {
                operation_id: reserve.operation_id,
                site_id: reserve.site_id,
                poi_id: reserve.poi_id,
                expected_site_revision: reserve.expected_site_revision,
            }
        }
        wire::SettlementOperation::ReleaseResidentSite(release) => {
            ScriptSettlementOperation::ReleaseResidentSite {
                operation_id: release.operation_id,
                spawn_site_token: release.spawn_site_token,
            }
        }
        wire::SettlementOperation::Survey(survey) => ScriptSettlementOperation::Survey {
            dimension: survey.dimension,
            bounds: bounds(survey.bounds)?,
            purpose: purpose(survey.purpose),
        },
        wire::SettlementOperation::PrepareStructure(prepare) => {
            ScriptSettlementOperation::PrepareStructure {
                operation_id: prepare.operation_id,
                blueprint_id: prepare.blueprint_id,
                anchor: position(prepare.anchor),
                rotation: prepare.rotation,
                survey_token: prepare.survey_token,
                expected_site_revision: prepare.expected_site_revision,
            }
        }
        wire::SettlementOperation::AdvanceStructure(advance) => {
            ScriptSettlementOperation::AdvanceStructure {
                operation_id: advance.operation_id,
                structure_id: advance.structure_id,
                stage: advance.stage,
                reservation_ref: advance.reservation_ref,
                expected_revision: advance.expected_revision,
                work_units: advance.work_units,
            }
        }
        wire::SettlementOperation::PauseStructure(pause) => {
            ScriptSettlementOperation::PauseStructure {
                operation_id: pause.operation_id,
                structure_id: pause.structure_id,
                expected_revision: pause.expected_revision,
            }
        }
        wire::SettlementOperation::ResumeStructure(resume) => {
            ScriptSettlementOperation::ResumeStructure {
                operation_id: resume.operation_id,
                structure_id: resume.structure_id,
                expected_revision: resume.expected_revision,
            }
        }
        wire::SettlementOperation::CancelStructure(cancel) => {
            ScriptSettlementOperation::CancelStructure {
                operation_id: cancel.operation_id,
                structure_id: cancel.structure_id,
                expected_revision: cancel.expected_revision,
            }
        }
        wire::SettlementOperation::StructureStatus(status) => ScriptSettlementOperation::Status {
            structure_id: status.structure_id,
        },
        wire::SettlementOperation::BindWarehouse(bind) => {
            ScriptSettlementOperation::BindWarehouse {
                operation_id: bind.operation_id,
                structure_id: bind.structure_id,
                container_id: bind.container_id,
            }
        }
        wire::SettlementOperation::BindVillageWarehouse(bind) => {
            ScriptSettlementOperation::BindVillageWarehouse {
                operation_id: bind.operation_id,
                site_id: bind.site_id,
                container_id: bind.container_id,
            }
        }
    })
}

/// One settlement result, as the contract names it, or nothing when the contract
/// cannot express the server's own variant.
pub(crate) fn encode_result(value: &ScriptSettlementResult) -> Option<wire::SettlementResult> {
    Some(match value {
        ScriptSettlementResult::Sites { page } => wire::SettlementResult::Sites(site_page(page)?),
        ScriptSettlementResult::Site { site } => wire::SettlementResult::Site(site_snapshot(site)?),
        ScriptSettlementResult::ResidentSite { reservation } => {
            wire::SettlementResult::ResidentSite(reservation_snapshot(reservation))
        }
        ScriptSettlementResult::Survey { survey } => {
            wire::SettlementResult::Survey(survey_snapshot(survey)?)
        }
        ScriptSettlementResult::Structure { structure } => {
            wire::SettlementResult::Structure(structure_snapshot(structure)?)
        }
        ScriptSettlementResult::Receipt { receipt } => {
            wire::SettlementResult::Receipt(structure_receipt(receipt))
        }
        ScriptSettlementResult::Warehouse { binding } => {
            wire::SettlementResult::Warehouse(warehouse_binding(binding)?)
        }
        _ => return None,
    })
}

/// The longest text one settlement record carries, for the staging bound the host
/// checks a callback's answer against.
///
/// A record carries a few strings rather than one - a 64-byte operation id, site,
/// poi, structure or stage id, a 64-byte survey token, a 64-byte reservation
/// reference, a contract resource id - and the server's own DTO already bounds
/// every one of them, so the whole record is bounded by construction and charging
/// every string here would refuse a record the DTO admits. What this answers is
/// the longest single string the guest put in the record, exactly as the rest of a
/// staged batch is charged its longest string rather than its whole content. A
/// record with no string of its own - a survey names only its dimension - charges
/// that one.
pub(crate) fn max_text_bytes(value: &wire::SettlementOperation) -> usize {
    match value {
        wire::SettlementOperation::ListSites(list) => text_of(list.cursor.as_deref()),
        wire::SettlementOperation::QuerySite(query) => longest(&[
            query.site_id.as_str(),
            query.cursor.as_deref().unwrap_or(""),
        ]),
        wire::SettlementOperation::ReserveResidentSite(reserve) => longest(&[
            reserve.operation_id.as_str(),
            reserve.site_id.as_str(),
            reserve.poi_id.as_str(),
        ]),
        wire::SettlementOperation::ReleaseResidentSite(release) => longest(&[
            release.operation_id.as_str(),
            release.spawn_site_token.as_str(),
        ]),
        wire::SettlementOperation::Survey(survey) => survey.dimension.len(),
        wire::SettlementOperation::PrepareStructure(prepare) => longest(&[
            prepare.operation_id.as_str(),
            prepare.blueprint_id.as_str(),
            prepare.survey_token.as_str(),
        ]),
        wire::SettlementOperation::AdvanceStructure(advance) => longest(&[
            advance.operation_id.as_str(),
            advance.structure_id.as_str(),
            advance.stage.as_str(),
            advance.reservation_ref.as_str(),
        ]),
        wire::SettlementOperation::PauseStructure(pause) => {
            longest(&[pause.operation_id.as_str(), pause.structure_id.as_str()])
        }
        wire::SettlementOperation::ResumeStructure(resume) => {
            longest(&[resume.operation_id.as_str(), resume.structure_id.as_str()])
        }
        wire::SettlementOperation::CancelStructure(cancel) => {
            longest(&[cancel.operation_id.as_str(), cancel.structure_id.as_str()])
        }
        wire::SettlementOperation::StructureStatus(status) => status.structure_id.len(),
        wire::SettlementOperation::BindWarehouse(bind) => {
            longest(&[bind.operation_id.as_str(), bind.structure_id.as_str()])
        }
        wire::SettlementOperation::BindVillageWarehouse(bind) => {
            longest(&[bind.operation_id.as_str(), bind.site_id.as_str()])
        }
    }
}

/// The longest of a record's own strings, or zero when it carries none.
fn longest(values: &[&str]) -> usize {
    values.iter().map(|value| value.len()).max().unwrap_or(0)
}

/// The length of an optional string, or zero when absent.
fn text_of(value: Option<&str>) -> usize {
    value.map_or(0, str::len)
}

/// One contract coordinate as the server's own integer triple.
fn position(value: wire::BlockPosition) -> [i32; 3] {
    [value.x, value.y, value.z]
}

/// One server coordinate as the contract's own record.
fn contract_position(value: [i32; 3]) -> wire::BlockPosition {
    wire::BlockPosition {
        x: value[0],
        y: value[1],
        z: value[2],
    }
}

/// One contract survey bounds as the server's own, through the DTO's own
/// constructor.
fn bounds(value: wire::SurveyBounds) -> Result<ScriptSurveyBounds, ScriptDtoError> {
    ScriptSurveyBounds::new(position(value.min), position(value.max))
}

/// One contract survey bounds as the contract names it. Bounds the server holds
/// are already an inclusive pair of corners, so this is a rename.
fn contract_bounds(value: &ScriptSurveyBounds) -> wire::SurveyBounds {
    wire::SurveyBounds {
        min: contract_position(value.min()),
        max: contract_position(value.max()),
    }
}

/// The server's own survey purpose the contract names.
fn purpose(value: wire::SurveyPurpose) -> ScriptSurveyPurpose {
    match value {
        wire::SurveyPurpose::Settlement => ScriptSurveyPurpose::Settlement,
        wire::SurveyPurpose::Expansion => ScriptSurveyPurpose::Expansion,
        wire::SurveyPurpose::Restoration => ScriptSurveyPurpose::Restoration,
    }
}

/// One server site variant as the contract names it, or nothing for a variant this
/// contract does not name yet.
fn site_variant(value: ScriptSiteVariant) -> Option<wire::SiteVariant> {
    Some(match value {
        ScriptSiteVariant::Hamlet => wire::SiteVariant::Hamlet,
        ScriptSiteVariant::Village => wire::SiteVariant::Village,
        ScriptSiteVariant::Town => wire::SiteVariant::Town,
        _ => return None,
    })
}

/// One server provenance as the contract names it.
fn provenance(value: ScriptSiteProvenance) -> Option<wire::SiteProvenance> {
    Some(match value {
        ScriptSiteProvenance::Authored => wire::SiteProvenance::Authored,
        ScriptSiteProvenance::VanillaVillage => wire::SiteProvenance::VanillaVillage,
        _ => return None,
    })
}

/// One server point-of-interest role as the contract names it.
fn poi_kind(value: ScriptSitePoiKind) -> Option<wire::SitePoiKind> {
    Some(match value {
        ScriptSitePoiKind::Home => wire::SitePoiKind::Home,
        ScriptSitePoiKind::Work => wire::SitePoiKind::Work,
        ScriptSitePoiKind::Meeting => wire::SitePoiKind::Meeting,
        ScriptSitePoiKind::Guard => wire::SitePoiKind::Guard,
        _ => return None,
    })
}

/// One server point-of-interest state as the contract names it.
fn poi_state(value: ScriptSitePoiState) -> Option<wire::SitePoiState> {
    Some(match value {
        ScriptSitePoiState::Free => wire::SitePoiState::Free,
        ScriptSitePoiState::Reserved => wire::SitePoiState::Reserved,
        ScriptSitePoiState::Occupied => wire::SitePoiState::Occupied,
        _ => return None,
    })
}

/// One server chunk availability as the contract names it.
fn chunk_availability(value: ScriptChunkAvailability) -> Option<wire::ChunkAvailability> {
    Some(match value {
        ScriptChunkAvailability::Loaded => wire::ChunkAvailability::Loaded,
        ScriptChunkAvailability::Unloaded => wire::ChunkAvailability::Unloaded,
        _ => return None,
    })
}

/// One server structure state as the contract names it.
fn structure_state(value: ScriptStructureState) -> Option<wire::StructureState> {
    Some(match value {
        ScriptStructureState::Prepared => wire::StructureState::Prepared,
        ScriptStructureState::Running => wire::StructureState::Running,
        ScriptStructureState::Paused => wire::StructureState::Paused,
        ScriptStructureState::Committed => wire::StructureState::Committed,
        ScriptStructureState::Cancelled => wire::StructureState::Cancelled,
        _ => return None,
    })
}

/// One server site page as the contract names it.
fn site_page(value: &ScriptSettlementSitePage) -> Option<wire::SettlementSitePage> {
    Some(wire::SettlementSitePage {
        sites: value
            .sites
            .iter()
            .map(site_snapshot)
            .collect::<Option<Vec<_>>>()?,
        cursor: value.cursor.clone(),
    })
}

/// One server site as the contract names it.
fn site_snapshot(value: &ScriptSettlementSite) -> Option<wire::SettlementSite> {
    Some(wire::SettlementSite {
        site_id: value.site_id.clone(),
        provenance: provenance(value.provenance)?,
        variant: site_variant(value.variant)?,
        revision: value.revision,
        contents_known: value.contents_known,
        footprint_origin: contract_position(value.footprint_origin),
        footprint_size: contract_position(value.footprint_size),
        buildings: value.buildings.iter().map(building).collect(),
        pois: value.pois.iter().map(poi).collect::<Option<Vec<_>>>()?,
        inhabitant_generation_ids: value.inhabitant_generation_ids.clone(),
    })
}

/// One server blueprint placement as the contract names it.
fn building(value: &ScriptSettlementBuilding) -> wire::SettlementBuilding {
    wire::SettlementBuilding {
        blueprint_id: value.blueprint_id.clone(),
        origin: contract_position(value.origin),
        rotation: value.rotation,
    }
}

/// One server point of interest as the contract names it.
fn poi(value: &ScriptSettlementPoi) -> Option<wire::SettlementPoi> {
    Some(wire::SettlementPoi {
        poi_id: value.poi_id.clone(),
        kind: poi_kind(value.kind)?,
        at: contract_position(value.at),
        capacity: value.capacity,
        state: poi_state(value.state)?,
    })
}

/// One server reservation as the contract names it.
fn reservation_snapshot(value: &ScriptResidentSiteReservation) -> wire::ResidentSiteReservation {
    wire::ResidentSiteReservation {
        site_id: value.site_id.clone(),
        poi_id: value.poi_id.clone(),
        spawn_site_token: value.spawn_site_token.clone(),
        revision: value.revision,
    }
}

/// One server survey as the contract names it.
fn survey_snapshot(value: &ScriptSurveySnapshot) -> Option<wire::SurveySnapshot> {
    Some(wire::SurveySnapshot {
        dimension: value.dimension.clone(),
        bounds: contract_bounds(&value.bounds),
        revision: value.revision,
        chunk_availability: chunk_availability(value.chunk_availability)?,
        survey_token: value.survey_token.clone(),
        usable_plots: value.usable_plots,
        water_columns: value.water_columns,
        claimed: value.claimed,
        existing_structures: value.existing_structures,
        biome_tags: value.biome_tags.clone(),
        resource_tags: value.resource_tags.clone(),
    })
}

/// One server material requirement as the contract names it.
fn material(value: &ScriptStructureMaterial) -> wire::StructureMaterial {
    wire::StructureMaterial {
        resource: value.resource.clone(),
        quantity: value.quantity,
    }
}

/// One server stage plan as the contract names it.
fn stage_plan(value: &ScriptStructureStagePlan) -> wire::StructureStagePlan {
    wire::StructureStagePlan {
        stage: value.stage.clone(),
        block_count: value.block_count,
        work_units: value.work_units,
        materials: value.materials.iter().map(material).collect(),
    }
}

/// One server portion receipt as the contract names it.
fn structure_receipt(value: &ScriptStructureReceipt) -> wire::StructureReceipt {
    wire::StructureReceipt {
        structure_id: value.structure_id.clone(),
        stage: value.stage.clone(),
        sequence: value.sequence,
        block_count: value.block_count,
        work_units: value.work_units,
        consumed: value.consumed.iter().map(material).collect(),
        revision: value.revision,
    }
}

/// One server structure snapshot as the contract names it.
fn structure_snapshot(value: &ScriptStructureSnapshot) -> Option<wire::StructureSnapshot> {
    Some(wire::StructureSnapshot {
        structure_id: value.structure_id.clone(),
        blueprint_id: value.blueprint_id.clone(),
        site_id: value.site_id.clone(),
        state: structure_state(value.state)?,
        revision: value.revision,
        origin: contract_position(value.origin),
        rotation: value.rotation,
        reserved_footprint: contract_position(value.reserved_footprint),
        stages: value.stages.iter().map(stage_plan).collect(),
        current_stage_index: value.current_stage_index,
        completed_work_units: value.completed_work_units,
        resource_plan_hash: value.resource_plan_hash.clone(),
        reservation_ref: value.reservation_ref.clone(),
        watermark: value.watermark,
        consumed: value.consumed.iter().map(material).collect(),
        remaining: value.remaining.iter().map(material).collect(),
        pause_reason: value.pause_reason.clone(),
    })
}

/// One server warehouse binding as the contract names it.
fn warehouse_binding(value: &ScriptWarehouseBinding) -> Option<wire::WarehouseBinding> {
    Some(wire::WarehouseBinding {
        handle: value.handle.clone(),
        source: warehouse_source(&value.source)?,
        revision: value.revision,
    })
}

fn warehouse_source(value: &ScriptWarehouseSource) -> Option<wire::WarehouseSource> {
    Some(match value {
        ScriptWarehouseSource::Authored {
            structure_id,
            container_id,
        } => wire::WarehouseSource::Authored(wire::AuthoredWarehouseSource {
            structure_id: structure_id.clone(),
            container_id: *container_id,
        }),
        ScriptWarehouseSource::VanillaVillage {
            site_id,
            container_id,
        } => wire::WarehouseSource::VanillaVillage(wire::VillageWarehouseSource {
            site_id: site_id.clone(),
            container_id: *container_id,
        }),
        _ => return None,
    })
}
