use std::sync::{Arc, Mutex};

use mlua::serde::SerializeOptions;
use mlua::{Lua, LuaSerdeExt, LuaString, Table, Value};

use super::{
    DiskManifest, InvocationState, bounded_lua_string, bounded_script_id, dto_error,
    lua_input_error, parse_storage_mutations, push_command, raw_bounded_string_field,
    raw_i32_field, raw_table_entry, raw_u8_field, raw_u16_field, raw_u32_field, raw_u64_field,
    validate_record_shape, validate_sequence_shape,
};
use crate::{
    MAX_BLUEPRINT_ID_BYTES, MAX_INVENTORY_RESOURCE_TYPES, MAX_INVENTORY_WORK_PORTIONS,
    MAX_ORDER_AFFILIATIONS, MAX_ORDER_TARGETS, MAX_ORDER_WAYPOINTS, MAX_OWNED_INVENTORY_TRANSFERS,
    MAX_PLUGIN_STORAGE_KEY_BYTES, MAX_RECIPE_ID_BYTES, MAX_RESIDENT_ENTITY_UUID_BYTES,
    MAX_RESIDENT_HANDLE_BYTES, MAX_RESIDENT_ORDER_HANDLES, MAX_RESIDENT_POI_HANDLE_BYTES,
    MAX_RESIDENT_QUERY_HANDLES, MAX_RESIDENT_SPAWN_TOKEN_BYTES, MAX_SCRIPT_ID_BYTES,
    MAX_SCRIPT_RESOURCE_ID_BYTES, MAX_SETTLEMENT_SITE_PAGE, MAX_SITE_ID_BYTES,
    MAX_STORAGE_SCAN_PAGE, MAX_STRUCTURE_ID_BYTES, MAX_SURVEY_TOKEN_BYTES, MAX_TARGET_REF_BYTES,
    MAX_WAREHOUSE_HANDLE_BYTES, ScriptBlockPosition, ScriptCommand, ScriptEngagementPolicy,
    ScriptFormation, ScriptFormationKind, ScriptHostileCategory, ScriptInventoryEndpoint,
    ScriptInventoryExpectedRevision, ScriptInventoryFence, ScriptInventoryMaterial,
    ScriptInventoryResourcePlan, ScriptInventoryWorkPortion, ScriptOperation,
    ScriptOperationOutcome, ScriptOperationRequest, ScriptOrderTargetRef,
    ScriptOwnedInventoryOperation, ScriptOwnedItemTransfer, ScriptResidentKind,
    ScriptResidentOperation, ScriptResidentOrder, ScriptResidentOrderOperation,
    ScriptResidentProfile, ScriptResidentWorkOrder, ScriptSettlementOperation, ScriptSurveyBounds,
    ScriptSurveyPurpose, ScriptWorkArea,
};

/// Capability names that a package must also declare in `required_features`.
const FEATURE_CAPABILITIES: [&str; 7] = [
    "storage_batches",
    "inventory_transfers",
    "persistent_residents",
    "resident_work",
    "resident_orders",
    "world_sites",
    "structure_operations",
];

pub(super) fn validate_required_features(disk: &DiskManifest) -> Result<(), String> {
    if disk.required_features.len() > crate::MAX_MANIFEST_CAPABILITIES {
        return Err("too many required plugin features".to_owned());
    }
    for feature in &disk.required_features {
        crate::validate_script_id_value(feature).map_err(|error| error.to_string())?;
        if !FEATURE_CAPABILITIES.contains(&feature.as_str()) {
            return Err(format!("unsupported required plugin feature {feature:?}"));
        }
    }
    for feature in FEATURE_CAPABILITIES {
        if disk
            .capabilities
            .iter()
            .any(|capability| capability == feature)
            && !disk
                .required_features
                .iter()
                .any(|declared| declared == feature)
        {
            return Err(format!(
                "{feature} capability requires required_features = [\"{feature}\"]"
            ));
        }
    }
    Ok(())
}

pub(super) fn install(
    lua: &Lua,
    api: &Table,
    invocation: &Arc<Mutex<Option<InvocationState>>>,
) -> mlua::Result<()> {
    let batch_invocation = Arc::clone(invocation);
    api.set(
        "storage_batch_cas",
        lua.create_function(
            move |_, (request_id, operation_id, mutations): (LuaString, LuaString, Table)| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::StorageBatch {
                        operation_id: bounded_script_id(operation_id, "operation_id")?,
                        mutations: parse_storage_mutations(mutations)?,
                    },
                )
                .map_err(dto_error)?;
                push_command(&batch_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let scan_invocation = Arc::clone(invocation);
    api.set(
        "storage_scan",
        lua.create_function(
            move |_, (request_id, prefix, cursor, limit): (LuaString, LuaString, Value, Value)| {
                let cursor = match cursor {
                    Value::Nil => None,
                    Value::String(cursor) => Some(bounded_lua_string(
                        cursor,
                        "storage_scan_cursor",
                        MAX_SCRIPT_ID_BYTES,
                        false,
                    )?),
                    _ => return Err(lua_input_error("storage_scan_cursor", "type")),
                };
                let limit = match limit {
                    Value::Integer(limit)
                        if (1..=MAX_STORAGE_SCAN_PAGE as i64).contains(&limit) =>
                    {
                        limit as u8
                    }
                    _ => return Err(lua_input_error("storage_scan_limit", "range")),
                };
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::StorageScan {
                        prefix: bounded_lua_string(
                            prefix,
                            "storage_scan_prefix",
                            MAX_PLUGIN_STORAGE_KEY_BYTES,
                            true,
                        )?,
                        cursor,
                        limit,
                    },
                )
                .map_err(dto_error)?;
                push_command(&scan_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let status_invocation = Arc::clone(invocation);
    api.set(
        "operation_status",
        lua.create_function(
            move |_, (request_id, operation_id): (LuaString, LuaString)| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Status {
                        operation_id: bounded_script_id(operation_id, "operation_id")?,
                    },
                )
                .map_err(dto_error)?;
                push_command(&status_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let query_invocation = Arc::clone(invocation);
    api.set(
        "query_owned_inventory",
        lua.create_function(
            move |_, (request_id, endpoint, expected_revision): (LuaString, Table, Value)| {
                let expected_revision = match expected_revision {
                    Value::Nil => None,
                    Value::Integer(revision) => Some(
                        u64::try_from(revision)
                            .map_err(|_| lua_input_error("inventory_expected_revision", "range"))?,
                    ),
                    _ => return Err(lua_input_error("inventory_expected_revision", "type")),
                };
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Inventory {
                        operation: ScriptOwnedInventoryOperation::Query {
                            endpoint: parse_inventory_endpoint(&endpoint)?,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(&query_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let transfer_invocation = Arc::clone(invocation);
    api.set(
        "transfer_owned_items",
        lua.create_function(
            move |_,
                  (request_id, operation_id, actor_id, transfers, expected_revisions): (
                LuaString,
                LuaString,
                u64,
                Table,
                Table,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Inventory {
                        operation: ScriptOwnedInventoryOperation::Transfer {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            actor_id,
                            transfers: parse_owned_transfers(&transfers)?,
                            expected_revisions: parse_expected_revisions(&expected_revisions)?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(&transfer_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let reserve_invocation = Arc::clone(invocation);
    api.set(
        "reserve_inventory_items",
        lua.create_function(
            move |_,
                  (request_id, operation_id, endpoint, resource_plan, expected_revision): (
                LuaString,
                LuaString,
                Table,
                Table,
                Table,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Inventory {
                        operation: ScriptOwnedInventoryOperation::Reserve {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            endpoint: parse_inventory_endpoint(&endpoint)?,
                            resource_plan: parse_resource_plan(&resource_plan)?,
                            expected_revision: parse_inventory_fence(&expected_revision)?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(&reserve_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let reservation_status_invocation = Arc::clone(invocation);
    api.set(
        "inventory_reservation_status",
        lua.create_function(
            move |_, (request_id, reservation_ref): (LuaString, LuaString)| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Inventory {
                        operation: ScriptOwnedInventoryOperation::ReservationStatus {
                            reservation_ref: bounded_lua_string(
                                reservation_ref,
                                "inventory_reservation",
                                64,
                                false,
                            )?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &reservation_status_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let release_invocation = Arc::clone(invocation);
    api.set(
        "release_inventory_reservation",
        lua.create_function(
            move |_,
                  (request_id, operation_id, reservation_ref, expected_revision): (
                LuaString,
                LuaString,
                LuaString,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Inventory {
                        operation: ScriptOwnedInventoryOperation::Release {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            reservation_ref: bounded_lua_string(
                                reservation_ref,
                                "inventory_reservation",
                                64,
                                false,
                            )?,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(&release_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let claim_resident_invocation = Arc::clone(invocation);
    api.set(
        "claim_resident",
        lua.create_function(
            move |_,
                  (request_id, operation_id, actor_id, entity_uuid, expected_entity_revision): (
                LuaString,
                LuaString,
                u64,
                LuaString,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Resident {
                        operation: ScriptResidentOperation::Claim {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            actor_id,
                            entity_uuid: bounded_lua_string(
                                entity_uuid,
                                "resident_entity_uuid",
                                MAX_RESIDENT_ENTITY_UUID_BYTES,
                                false,
                            )?,
                            expected_entity_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &claim_resident_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let spawn_resident_invocation = Arc::clone(invocation);
    api.set(
        "spawn_resident",
        lua.create_function(
            move |_,
                  (request_id, operation_id, spawn_site_token, profile): (
                LuaString,
                LuaString,
                LuaString,
                Table,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Resident {
                        operation: ScriptResidentOperation::Spawn {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            spawn_site_token: bounded_lua_string(
                                spawn_site_token,
                                "spawn_site_token",
                                MAX_RESIDENT_SPAWN_TOKEN_BYTES,
                                false,
                            )?,
                            profile: parse_resident_profile(&profile)?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &spawn_resident_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let query_residents_invocation = Arc::clone(invocation);
    api.set(
        "query_residents",
        lua.create_function(
            move |_, (request_id, handles, cursor): (LuaString, Table, Value)| {
                let cursor = match cursor {
                    Value::Nil => None,
                    Value::String(cursor) => Some(bounded_lua_string(
                        cursor,
                        "resident_handle",
                        MAX_RESIDENT_HANDLE_BYTES,
                        false,
                    )?),
                    _ => return Err(lua_input_error("resident_cursor", "type")),
                };
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Resident {
                        operation: ScriptResidentOperation::Query {
                            handles: parse_resident_handles(&handles)?,
                            cursor,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &query_residents_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let release_resident_invocation = Arc::clone(invocation);
    api.set(
        "release_resident",
        lua.create_function(
            move |_,
                  (request_id, operation_id, handle, expected_revision): (
                LuaString,
                LuaString,
                LuaString,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Resident {
                        operation: ScriptResidentOperation::Release {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            handle: bounded_lua_string(
                                handle,
                                "resident_handle",
                                MAX_RESIDENT_HANDLE_BYTES,
                                false,
                            )?,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &release_resident_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let set_resident_pois_invocation = Arc::clone(invocation);
    api.set(
        "set_resident_pois",
        lua.create_function(
            move |_,
                  (
                request_id,
                operation_id,
                handle,
                home_poi,
                work_poi,
                meeting_poi,
                expected_revision,
            ): (LuaString, LuaString, LuaString, Value, Value, Value, u64)| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Resident {
                        operation: ScriptResidentOperation::SetPois {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            handle: bounded_lua_string(
                                handle,
                                "resident_handle",
                                MAX_RESIDENT_HANDLE_BYTES,
                                false,
                            )?,
                            home_poi: parse_resident_poi(home_poi, "resident_home_poi")?,
                            work_poi: parse_resident_poi(work_poi, "resident_work_poi")?,
                            meeting_poi: parse_resident_poi(meeting_poi, "resident_meeting_poi")?,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &set_resident_pois_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let list_sites_invocation = Arc::clone(invocation);
    api.set(
        "list_settlement_sites",
        lua.create_function(
            move |_, (request_id, cursor, limit): (LuaString, Value, Value)| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::ListSites {
                            cursor: parse_settlement_cursor(cursor)?,
                            limit: parse_settlement_limit(limit)?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(&list_sites_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let query_site_invocation = Arc::clone(invocation);
    api.set(
        "query_settlement_site",
        lua.create_function(
            move |_, (request_id, site_id, cursor, limit): (LuaString, LuaString, Value, Value)| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::QuerySite {
                            site_id: bounded_lua_string(
                                site_id,
                                "site_id",
                                MAX_SITE_ID_BYTES,
                                false,
                            )?,
                            cursor: parse_settlement_cursor(cursor)?,
                            limit: parse_settlement_limit(limit)?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(&query_site_invocation, ScriptCommand::Operation { request })
            },
        )?,
    )?;

    let reserve_resident_site_invocation = Arc::clone(invocation);
    api.set(
        "reserve_resident_site",
        lua.create_function(
            move |_,
                  (request_id, operation_id, site_id, poi_id, expected_site_revision): (
                LuaString,
                LuaString,
                LuaString,
                LuaString,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::ReserveResidentSite {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            site_id: bounded_lua_string(
                                site_id,
                                "site_id",
                                MAX_SITE_ID_BYTES,
                                false,
                            )?,
                            poi_id: bounded_lua_string(poi_id, "poi_id", MAX_SITE_ID_BYTES, false)?,
                            expected_site_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &reserve_resident_site_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let release_resident_site_invocation = Arc::clone(invocation);
    api.set(
        "release_resident_site",
        lua.create_function(
            move |_,
                  (request_id, operation_id, spawn_site_token): (
                LuaString,
                LuaString,
                LuaString,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::ReleaseResidentSite {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            spawn_site_token: bounded_lua_string(
                                spawn_site_token,
                                "spawn_site_token",
                                MAX_RESIDENT_SPAWN_TOKEN_BYTES,
                                false,
                            )?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &release_resident_site_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let survey_site_invocation = Arc::clone(invocation);
    api.set(
        "survey_site",
        lua.create_function(
            move |_,
                  (request_id, dimension, bounds, purpose): (
                LuaString,
                LuaString,
                Table,
                LuaString,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::Survey {
                            dimension: bounded_lua_string(
                                dimension,
                                "dimension",
                                MAX_SCRIPT_RESOURCE_ID_BYTES,
                                false,
                            )?,
                            bounds: parse_survey_bounds(&bounds)?,
                            purpose: parse_survey_purpose(purpose)?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &survey_site_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let prepare_structure_invocation = Arc::clone(invocation);
    api.set(
        "prepare_structure",
        lua.create_function(
            move |_,
                  (
                request_id,
                operation_id,
                blueprint_id,
                anchor,
                rotation,
                survey_token,
                expected_site_revision,
            ): (
                LuaString,
                LuaString,
                LuaString,
                Table,
                Value,
                LuaString,
                u64,
            )| {
                let rotation = match rotation {
                    Value::Integer(rotation) => u16::try_from(rotation)
                        .map_err(|_| lua_input_error("structure_rotation", "range"))?,
                    _ => return Err(lua_input_error("structure_rotation", "type")),
                };
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::PrepareStructure {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            blueprint_id: bounded_lua_string(
                                blueprint_id,
                                "blueprint_id",
                                MAX_BLUEPRINT_ID_BYTES,
                                false,
                            )?,
                            anchor: parse_coordinate(&anchor, "anchor")?,
                            rotation,
                            survey_token: bounded_lua_string(
                                survey_token,
                                "survey_token",
                                MAX_SURVEY_TOKEN_BYTES,
                                false,
                            )?,
                            expected_site_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &prepare_structure_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let advance_structure_invocation = Arc::clone(invocation);
    api.set(
        "advance_structure",
        lua.create_function(
            move |_,
                  (
                request_id,
                operation_id,
                structure_id,
                stage,
                reservation_ref,
                expected_revision,
                work_units,
            ): (
                LuaString,
                LuaString,
                LuaString,
                LuaString,
                LuaString,
                u64,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::AdvanceStructure {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            structure_id: bounded_lua_string(
                                structure_id,
                                "structure_id",
                                MAX_STRUCTURE_ID_BYTES,
                                false,
                            )?,
                            stage: bounded_lua_string(
                                stage,
                                "structure_stage",
                                MAX_STRUCTURE_ID_BYTES,
                                false,
                            )?,
                            reservation_ref: bounded_lua_string(
                                reservation_ref,
                                "reservation_ref",
                                64,
                                false,
                            )?,
                            expected_revision,
                            work_units,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &advance_structure_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let pause_structure_invocation = Arc::clone(invocation);
    api.set(
        "pause_structure",
        lua.create_function(
            move |_,
                  (request_id, operation_id, structure_id, expected_revision): (
                LuaString,
                LuaString,
                LuaString,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::PauseStructure {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            structure_id: bounded_lua_string(
                                structure_id,
                                "structure_id",
                                MAX_STRUCTURE_ID_BYTES,
                                false,
                            )?,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &pause_structure_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let cancel_structure_invocation = Arc::clone(invocation);
    api.set(
        "cancel_structure",
        lua.create_function(
            move |_,
                  (request_id, operation_id, structure_id, expected_revision): (
                LuaString,
                LuaString,
                LuaString,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::CancelStructure {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            structure_id: bounded_lua_string(
                                structure_id,
                                "structure_id",
                                MAX_STRUCTURE_ID_BYTES,
                                false,
                            )?,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &cancel_structure_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let structure_status_invocation = Arc::clone(invocation);
    api.set(
        "structure_status",
        lua.create_function(
            move |_, (request_id, structure_id): (LuaString, LuaString)| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::Status {
                            structure_id: bounded_lua_string(
                                structure_id,
                                "structure_id",
                                MAX_STRUCTURE_ID_BYTES,
                                false,
                            )?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &structure_status_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let bind_warehouse_invocation = Arc::clone(invocation);
    api.set(
        "bind_warehouse",
        lua.create_function(
            move |_,
                  (request_id, operation_id, structure_id, container_id): (
                LuaString,
                LuaString,
                LuaString,
                u32,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::Settlement {
                        operation: ScriptSettlementOperation::BindWarehouse {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            structure_id: bounded_lua_string(
                                structure_id,
                                "structure_id",
                                MAX_STRUCTURE_ID_BYTES,
                                false,
                            )?,
                            container_id,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &bind_warehouse_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let assign_resident_work_invocation = Arc::clone(invocation);
    api.set(
        "assign_resident_work",
        lua.create_function(
            move |_,
                  (request_id, operation_id, handle, work, work_units, expected_revision): (
                LuaString,
                LuaString,
                LuaString,
                Table,
                u64,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::ResidentOrder {
                        operation: ScriptResidentOrderOperation::AssignWork {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            handle: bounded_lua_string(
                                handle,
                                "resident_handle",
                                MAX_RESIDENT_HANDLE_BYTES,
                                false,
                            )?,
                            work: parse_work_order(&work)?,
                            work_units,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &assign_resident_work_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let cancel_resident_work_invocation = Arc::clone(invocation);
    api.set(
        "cancel_resident_work",
        lua.create_function(
            move |_,
                  (request_id, operation_id, handle, expected_revision): (
                LuaString,
                LuaString,
                LuaString,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::ResidentOrder {
                        operation: ScriptResidentOrderOperation::CancelWork {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            handle: bounded_lua_string(
                                handle,
                                "resident_handle",
                                MAX_RESIDENT_HANDLE_BYTES,
                                false,
                            )?,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &cancel_resident_work_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let issue_resident_order_invocation = Arc::clone(invocation);
    api.set(
        "issue_resident_order",
        lua.create_function(
            move |_,
                  (request_id, operation_id, handles, expected_order_revisions, order): (
                LuaString,
                LuaString,
                Table,
                Table,
                Table,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::ResidentOrder {
                        operation: ScriptResidentOrderOperation::IssueOrder {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            handles: parse_order_handles(&handles, "order_handles")?,
                            expected_order_revisions: parse_order_revisions(
                                &expected_order_revisions,
                                "order_revisions",
                            )?,
                            order: parse_resident_order(&order)?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &issue_resident_order_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let cancel_resident_order_invocation = Arc::clone(invocation);
    api.set(
        "cancel_resident_order",
        lua.create_function(
            move |_,
                  (request_id, operation_id, handles, expected_order_revisions): (
                LuaString,
                LuaString,
                Table,
                Table,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::ResidentOrder {
                        operation: ScriptResidentOrderOperation::CancelOrder {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            handles: parse_order_handles(&handles, "order_handles")?,
                            expected_order_revisions: parse_order_revisions(
                                &expected_order_revisions,
                                "order_revisions",
                            )?,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &cancel_resident_order_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    let demobilize_resident_invocation = Arc::clone(invocation);
    api.set(
        "demobilize_resident",
        lua.create_function(
            move |_,
                  (request_id, operation_id, handle, expected_revision): (
                LuaString,
                LuaString,
                LuaString,
                u64,
            )| {
                let request = ScriptOperationRequest::try_new(
                    bounded_script_id(request_id, "request_id")?,
                    ScriptOperation::ResidentOrder {
                        operation: ScriptResidentOrderOperation::Demobilize {
                            operation_id: bounded_script_id(operation_id, "operation_id")?,
                            handle: bounded_lua_string(
                                handle,
                                "resident_handle",
                                MAX_RESIDENT_HANDLE_BYTES,
                                false,
                            )?,
                            expected_revision,
                        },
                    },
                )
                .map_err(dto_error)?;
                push_command(
                    &demobilize_resident_invocation,
                    ScriptCommand::Operation { request },
                )
            },
        )?,
    )?;

    Ok(())
}

fn parse_block_position(table: &Table, field: &'static str) -> mlua::Result<ScriptBlockPosition> {
    let [x, y, z] = parse_coordinate(table, field)?;
    Ok(ScriptBlockPosition::new(x, y, z))
}

fn parse_work_area(table: &Table) -> mlua::Result<ScriptWorkArea> {
    validate_record_shape(table, &["dimension", "min", "max"], "work_area")?;
    let dimension = raw_bounded_string_field(
        table,
        "dimension",
        "work_dimension",
        MAX_SCRIPT_RESOURCE_ID_BYTES,
        false,
    )?;
    let min = raw_named_table(table, "min", "work_area_min")?;
    let max = raw_named_table(table, "max", "work_area_max")?;
    Ok(ScriptWorkArea::new(
        dimension,
        parse_block_position(&min, "work_area_min")?,
        parse_block_position(&max, "work_area_max")?,
    ))
}

fn raw_named_table(table: &Table, key: &'static str, field: &'static str) -> mlua::Result<Table> {
    match table.raw_get::<Value>(key)? {
        Value::Table(value) => Ok(value),
        _ => Err(lua_input_error(field, "type")),
    }
}

fn parse_work_order(table: &Table) -> mlua::Result<ScriptResidentWorkOrder> {
    let kind = raw_bounded_string_field(table, "kind", "work_kind", 24, false)?;
    match kind.as_str() {
        "harvest" => {
            validate_record_shape(table, &["kind", "area", "tool"], "work_order")?;
            Ok(ScriptResidentWorkOrder::Harvest {
                area: parse_work_area(&raw_named_table(table, "area", "work_area")?)?,
                tool: raw_bounded_string_field(
                    table,
                    "tool",
                    "work_tool",
                    MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?,
            })
        }
        "replant" => {
            validate_record_shape(table, &["kind", "area", "seed", "tool"], "work_order")?;
            Ok(ScriptResidentWorkOrder::Replant {
                area: parse_work_area(&raw_named_table(table, "area", "work_area")?)?,
                seed: raw_bounded_string_field(
                    table,
                    "seed",
                    "work_seed",
                    MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?,
                tool: raw_bounded_string_field(
                    table,
                    "tool",
                    "work_tool",
                    MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?,
            })
        }
        "cut_tree" | "mine" | "fish" => {
            validate_record_shape(table, &["kind", "area", "tool"], "work_order")?;
            let area = parse_work_area(&raw_named_table(table, "area", "work_area")?)?;
            let tool = raw_bounded_string_field(
                table,
                "tool",
                "work_tool",
                MAX_SCRIPT_RESOURCE_ID_BYTES,
                false,
            )?;
            Ok(match kind.as_str() {
                "cut_tree" => ScriptResidentWorkOrder::CutTree { area, tool },
                "mine" => ScriptResidentWorkOrder::Mine { area, tool },
                _ => ScriptResidentWorkOrder::Fish { area, tool },
            })
        }
        "tend_livestock" => {
            validate_record_shape(table, &["kind", "area", "feed"], "work_order")?;
            Ok(ScriptResidentWorkOrder::TendLivestock {
                area: parse_work_area(&raw_named_table(table, "area", "work_area")?)?,
                feed: raw_bounded_string_field(
                    table,
                    "feed",
                    "work_feed",
                    MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?,
            })
        }
        "haul" => {
            validate_record_shape(table, &["kind", "source", "destination"], "work_order")?;
            Ok(ScriptResidentWorkOrder::Haul {
                source: parse_inventory_endpoint(&raw_named_table(
                    table,
                    "source",
                    "work_source",
                )?)?,
                destination: parse_inventory_endpoint(&raw_named_table(
                    table,
                    "destination",
                    "work_destination",
                )?)?,
            })
        }
        "craft" => {
            validate_record_shape(table, &["kind", "recipe", "count"], "work_order")?;
            Ok(ScriptResidentWorkOrder::Craft {
                recipe: raw_bounded_string_field(
                    table,
                    "recipe",
                    "work_recipe",
                    MAX_RECIPE_ID_BYTES,
                    false,
                )?,
                count: raw_u32_field(table, "count", "work_count")?,
            })
        }
        "construct" => {
            validate_record_shape(
                table,
                &["kind", "structure_id", "stage", "expected_revision"],
                "work_order",
            )?;
            Ok(ScriptResidentWorkOrder::Construct {
                structure_id: raw_bounded_string_field(
                    table,
                    "structure_id",
                    "structure_id",
                    MAX_STRUCTURE_ID_BYTES,
                    false,
                )?,
                stage: raw_bounded_string_field(
                    table,
                    "stage",
                    "structure_stage",
                    MAX_STRUCTURE_ID_BYTES,
                    false,
                )?,
                expected_revision: raw_u64_field(table, "expected_revision", "structure_revision")?,
            })
        }
        _ => Err(lua_input_error("work_kind", "invalid")),
    }
}

fn parse_formation(table: &Table) -> mlua::Result<ScriptFormation> {
    validate_record_shape(table, &["kind", "spacing"], "formation")?;
    let kind = match raw_bounded_string_field(table, "kind", "formation_kind", 8, false)?.as_str() {
        "line" => ScriptFormationKind::Line,
        "column" => ScriptFormationKind::Column,
        "wedge" => ScriptFormationKind::Wedge,
        "square" => ScriptFormationKind::Square,
        _ => return Err(lua_input_error("formation_kind", "invalid")),
    };
    Ok(ScriptFormation::new(
        kind,
        raw_u8_field(table, "spacing", "formation_spacing")?,
    ))
}

fn parse_resident_order(table: &Table) -> mlua::Result<ScriptResidentOrder> {
    let kind = raw_bounded_string_field(table, "kind", "order_kind", 16, false)?;
    match kind.as_str() {
        "follow" => {
            validate_record_shape(table, &["kind", "target_player", "formation"], "order")?;
            Ok(ScriptResidentOrder::Follow {
                target_player: raw_u64_field(table, "target_player", "order_player")?,
                formation: parse_formation(&raw_named_table(
                    table,
                    "formation",
                    "order_formation",
                )?)?,
            })
        }
        "move" => {
            validate_record_shape(
                table,
                &[
                    "kind",
                    "dimension",
                    "anchor",
                    "heading_degrees",
                    "formation",
                ],
                "order",
            )?;
            Ok(ScriptResidentOrder::Move {
                dimension: raw_bounded_string_field(
                    table,
                    "dimension",
                    "order_dimension",
                    MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?,
                anchor: parse_block_position(
                    &raw_named_table(table, "anchor", "order_anchor")?,
                    "order_anchor",
                )?,
                heading_degrees: raw_u16_field(table, "heading_degrees", "order_heading")?,
                formation: parse_formation(&raw_named_table(
                    table,
                    "formation",
                    "order_formation",
                )?)?,
            })
        }
        "hold" => {
            validate_record_shape(
                table,
                &[
                    "kind",
                    "anchor",
                    "heading_degrees",
                    "formation",
                    "engagement_radius",
                ],
                "order",
            )?;
            Ok(ScriptResidentOrder::Hold {
                anchor: parse_block_position(
                    &raw_named_table(table, "anchor", "order_anchor")?,
                    "order_anchor",
                )?,
                heading_degrees: raw_u16_field(table, "heading_degrees", "order_heading")?,
                formation: parse_formation(&raw_named_table(
                    table,
                    "formation",
                    "order_formation",
                )?)?,
                engagement_radius: raw_u8_field(
                    table,
                    "engagement_radius",
                    "order_engagement_radius",
                )?,
            })
        }
        "patrol" => {
            validate_record_shape(
                table,
                &["kind", "waypoints", "formation", "engagement_radius"],
                "order",
            )?;
            Ok(ScriptResidentOrder::Patrol {
                waypoints: parse_waypoints(&raw_named_table(
                    table,
                    "waypoints",
                    "order_waypoints",
                )?)?,
                formation: parse_formation(&raw_named_table(
                    table,
                    "formation",
                    "order_formation",
                )?)?,
                engagement_radius: raw_u8_field(
                    table,
                    "engagement_radius",
                    "order_engagement_radius",
                )?,
            })
        }
        "garrison" => {
            validate_record_shape(table, &["kind", "posts", "engagement_radius"], "order")?;
            Ok(ScriptResidentOrder::Garrison {
                posts: parse_order_handles(
                    &raw_named_table(table, "posts", "order_posts")?,
                    "order_posts",
                )?,
                engagement_radius: raw_u8_field(
                    table,
                    "engagement_radius",
                    "order_engagement_radius",
                )?,
            })
        }
        "attack" => {
            validate_record_shape(table, &["kind", "targets", "policy"], "order")?;
            Ok(ScriptResidentOrder::Attack {
                targets: parse_order_targets(&raw_named_table(table, "targets", "order_targets")?)?,
                policy: parse_engagement_policy(&raw_named_table(
                    table,
                    "policy",
                    "order_policy",
                )?)?,
            })
        }
        "retreat" => {
            validate_record_shape(table, &["kind", "anchor", "formation"], "order")?;
            Ok(ScriptResidentOrder::Retreat {
                anchor: parse_block_position(
                    &raw_named_table(table, "anchor", "order_anchor")?,
                    "order_anchor",
                )?,
                formation: parse_formation(&raw_named_table(
                    table,
                    "formation",
                    "order_formation",
                )?)?,
            })
        }
        _ => Err(lua_input_error("order_kind", "invalid")),
    }
}

fn parse_waypoints(table: &Table) -> mlua::Result<Vec<ScriptBlockPosition>> {
    let len = validate_sequence_shape(table, MAX_ORDER_WAYPOINTS, "order_waypoints")?;
    let mut waypoints = Vec::with_capacity(len);
    for index in 1..=len {
        let entry = raw_table_entry(table, index, "order_waypoint")?;
        waypoints.push(parse_block_position(&entry, "order_waypoint")?);
    }
    Ok(waypoints)
}

fn parse_order_handles(table: &Table, field: &'static str) -> mlua::Result<Vec<String>> {
    let len = validate_sequence_shape(table, MAX_RESIDENT_ORDER_HANDLES, field)?;
    let mut handles = Vec::with_capacity(len);
    for index in 1..=len {
        let handle = table.raw_get::<LuaString>(index)?;
        handles.push(bounded_lua_string(
            handle,
            "resident_handle",
            MAX_RESIDENT_HANDLE_BYTES,
            false,
        )?);
    }
    Ok(handles)
}

fn parse_order_revisions(table: &Table, field: &'static str) -> mlua::Result<Vec<u64>> {
    let len = validate_sequence_shape(table, MAX_RESIDENT_ORDER_HANDLES, field)?;
    let mut revisions = Vec::with_capacity(len);
    for index in 1..=len {
        match table.raw_get::<Value>(index)? {
            Value::Integer(value) => {
                let value = u64::try_from(value).map_err(|_| lua_input_error(field, "range"))?;
                revisions.push(value);
            }
            _ => return Err(lua_input_error(field, "type")),
        }
    }
    Ok(revisions)
}

fn parse_order_targets(table: &Table) -> mlua::Result<Vec<ScriptOrderTargetRef>> {
    let len = validate_sequence_shape(table, MAX_ORDER_TARGETS, "order_targets")?;
    let mut targets = Vec::with_capacity(len);
    for index in 1..=len {
        let entry = raw_table_entry(table, index, "order_target")?;
        validate_record_shape(
            &entry,
            &["target_ref", "policy_revision", "expires_revision"],
            "order_target",
        )?;
        targets.push(ScriptOrderTargetRef::new(
            raw_bounded_string_field(
                &entry,
                "target_ref",
                "target_ref",
                MAX_TARGET_REF_BYTES,
                false,
            )?,
            raw_u64_field(&entry, "policy_revision", "target_policy_revision")?,
            raw_u64_field(&entry, "expires_revision", "target_expires_revision")?,
        ));
    }
    Ok(targets)
}

fn parse_engagement_policy(table: &Table) -> mlua::Result<ScriptEngagementPolicy> {
    validate_record_shape(table, &["revision", "allies", "permitted"], "order_policy")?;
    let allies = parse_order_handles(
        &raw_named_table(table, "allies", "order_allies")?,
        "order_allies",
    )?;
    if allies.len() > MAX_ORDER_AFFILIATIONS {
        return Err(lua_input_error("order_allies", "range"));
    }
    let permitted_table = raw_named_table(table, "permitted", "order_permitted")?;
    let len = validate_sequence_shape(&permitted_table, 4, "order_permitted")?;
    let mut permitted = Vec::with_capacity(len);
    for index in 1..=len {
        let entry = permitted_table.raw_get::<LuaString>(index)?;
        let entry = bounded_lua_string(entry, "order_permitted", 16, false)?;
        permitted.push(match entry.as_str() {
            "hostile" => ScriptHostileCategory::Hostile,
            "player" => ScriptHostileCategory::Player,
            "owned_resident" => ScriptHostileCategory::OwnedResident,
            "neutral_animal" => ScriptHostileCategory::NeutralAnimal,
            _ => return Err(lua_input_error("order_permitted", "invalid")),
        });
    }
    Ok(ScriptEngagementPolicy::new(
        raw_u64_field(table, "revision", "order_policy_revision")?,
        allies,
        permitted,
    ))
}

fn parse_settlement_cursor(value: Value) -> mlua::Result<Option<String>> {
    match value {
        Value::Nil => Ok(None),
        Value::String(cursor) => Ok(Some(bounded_lua_string(
            cursor,
            "settlement_cursor",
            MAX_SITE_ID_BYTES,
            false,
        )?)),
        _ => Err(lua_input_error("settlement_cursor", "type")),
    }
}

fn parse_settlement_limit(value: Value) -> mlua::Result<u8> {
    match value {
        Value::Integer(limit) if (1..=MAX_SETTLEMENT_SITE_PAGE as i64).contains(&limit) => {
            Ok(limit as u8)
        }
        _ => Err(lua_input_error("settlement_limit", "range")),
    }
}

fn parse_survey_purpose(value: LuaString) -> mlua::Result<ScriptSurveyPurpose> {
    let purpose = bounded_lua_string(value, "survey_purpose", 11, false)?;
    match purpose.as_str() {
        "settlement" => Ok(ScriptSurveyPurpose::Settlement),
        "expansion" => Ok(ScriptSurveyPurpose::Expansion),
        "restoration" => Ok(ScriptSurveyPurpose::Restoration),
        _ => Err(lua_input_error("survey_purpose", "invalid")),
    }
}

fn parse_coordinate(table: &Table, field: &'static str) -> mlua::Result<[i32; 3]> {
    validate_record_shape(table, &["x", "y", "z"], field)?;
    Ok([
        raw_i32_field(table, "x", field)?,
        raw_i32_field(table, "y", field)?,
        raw_i32_field(table, "z", field)?,
    ])
}

fn parse_survey_bounds(table: &Table) -> mlua::Result<ScriptSurveyBounds> {
    validate_record_shape(table, &["min", "max"], "survey_bounds")?;
    let min = match table.raw_get::<Value>("min")? {
        Value::Table(min) => min,
        _ => return Err(lua_input_error("survey_bounds_min", "type")),
    };
    let max = match table.raw_get::<Value>("max")? {
        Value::Table(max) => max,
        _ => return Err(lua_input_error("survey_bounds_max", "type")),
    };
    ScriptSurveyBounds::new(
        parse_coordinate(&min, "survey_bounds_min")?,
        parse_coordinate(&max, "survey_bounds_max")?,
    )
    .map_err(dto_error)
}

fn parse_resident_handles(table: &Table) -> mlua::Result<Vec<String>> {
    let len = validate_sequence_shape(table, MAX_RESIDENT_QUERY_HANDLES, "resident_handles")?;
    let mut handles = Vec::with_capacity(len);
    for index in 1..=len {
        let handle = table.raw_get::<LuaString>(index)?;
        handles.push(bounded_lua_string(
            handle,
            "resident_handle",
            MAX_RESIDENT_HANDLE_BYTES,
            false,
        )?);
    }
    Ok(handles)
}

fn parse_resident_poi(value: Value, field: &'static str) -> mlua::Result<Option<String>> {
    match value {
        Value::Nil => Ok(None),
        Value::String(poi) => Ok(Some(bounded_lua_string(
            poi,
            field,
            MAX_RESIDENT_POI_HANDLE_BYTES,
            false,
        )?)),
        _ => Err(lua_input_error(field, "type")),
    }
}

fn parse_resident_profile(table: &Table) -> mlua::Result<ScriptResidentProfile> {
    validate_record_shape(table, &["kind"], "resident_profile")?;
    let kind = raw_bounded_string_field(table, "kind", "resident_kind", 16, false)?;
    match kind.as_str() {
        "villager" => Ok(ScriptResidentProfile::new(ScriptResidentKind::Villager)),
        _ => Err(lua_input_error("resident_kind", "invalid")),
    }
}

pub(super) fn set_result(
    lua: &Lua,
    table: &Table,
    request_id: &str,
    operation_id: Option<&str>,
    outcome: &ScriptOperationOutcome,
) -> mlua::Result<()> {
    table.set("request_id", request_id)?;
    table.set("operation_id", operation_id)?;
    table.set("state", outcome.state().as_str())?;
    table.set("revision", outcome.revision())?;
    table.set("failure", outcome.failure().map(|failure| failure.as_str()))?;
    table.set(
        "payload",
        lua.to_value_with(
            outcome.payload(),
            SerializeOptions::new().serialize_none_to_null(false),
        )?,
    )
}

fn parse_inventory_endpoint(table: &Table) -> mlua::Result<ScriptInventoryEndpoint> {
    let kind = raw_bounded_string_field(table, "kind", "inventory_endpoint_kind", 20, false)?;
    match kind.as_str() {
        "player_inventory" => {
            validate_record_shape(table, &["kind", "player_id"], "inventory_endpoint")?;
            Ok(ScriptInventoryEndpoint::PlayerInventory {
                player_id: raw_u64_field(table, "player_id", "inventory_player")?,
            })
        }
        "warehouse" => {
            validate_record_shape(table, &["kind", "handle"], "inventory_endpoint")?;
            Ok(ScriptInventoryEndpoint::Warehouse {
                handle: raw_bounded_string_field(
                    table,
                    "handle",
                    "warehouse_handle",
                    MAX_WAREHOUSE_HANDLE_BYTES,
                    false,
                )?,
            })
        }
        "resident_equipment" | "resident_carry" => {
            validate_record_shape(table, &["kind", "handle"], "inventory_endpoint")?;
            let handle = raw_bounded_string_field(
                table,
                "handle",
                "resident_handle",
                MAX_RESIDENT_HANDLE_BYTES,
                false,
            )?;
            if kind == "resident_equipment" {
                Ok(ScriptInventoryEndpoint::ResidentEquipment { handle })
            } else {
                Ok(ScriptInventoryEndpoint::ResidentCarry { handle })
            }
        }
        _ => Err(lua_input_error("inventory_endpoint_kind", "invalid")),
    }
}

fn parse_inventory_fence(table: &Table) -> mlua::Result<ScriptInventoryFence> {
    validate_record_shape(table, &["revision", "snapshot_hash"], "inventory_fence")?;
    ScriptInventoryFence::try_new(
        raw_u64_field(table, "revision", "inventory_revision")?,
        raw_bounded_string_field(table, "snapshot_hash", "inventory_snapshot_hash", 64, false)?,
    )
    .map_err(dto_error)
}

fn parse_owned_transfers(table: &Table) -> mlua::Result<Vec<ScriptOwnedItemTransfer>> {
    let len = validate_sequence_shape(table, MAX_OWNED_INVENTORY_TRANSFERS, "inventory_transfers")?;
    let mut transfers = Vec::with_capacity(len);
    for index in 1..=len {
        let transfer = raw_table_entry(table, index, "inventory_transfer")?;
        validate_record_shape(
            &transfer,
            &[
                "source",
                "source_slot",
                "destination",
                "destination_slot",
                "count",
            ],
            "inventory_transfer",
        )?;
        let source = match transfer.raw_get::<Value>("source")? {
            Value::Table(table) => table,
            _ => return Err(lua_input_error("inventory_source", "type")),
        };
        let destination = match transfer.raw_get::<Value>("destination")? {
            Value::Table(table) => table,
            _ => return Err(lua_input_error("inventory_destination", "type")),
        };
        transfers.push(ScriptOwnedItemTransfer::new(
            parse_inventory_endpoint(&source)?,
            raw_u8_field(&transfer, "source_slot", "inventory_source_slot")?,
            parse_inventory_endpoint(&destination)?,
            raw_u8_field(&transfer, "destination_slot", "inventory_destination_slot")?,
            raw_u32_field(&transfer, "count", "inventory_count")?,
        ));
    }
    Ok(transfers)
}

fn parse_expected_revisions(table: &Table) -> mlua::Result<Vec<ScriptInventoryExpectedRevision>> {
    let len = validate_sequence_shape(
        table,
        MAX_OWNED_INVENTORY_TRANSFERS * 2,
        "inventory_expected_revisions",
    )?;
    let mut revisions = Vec::with_capacity(len);
    for index in 1..=len {
        let entry = raw_table_entry(table, index, "inventory_expected_revision")?;
        validate_record_shape(
            &entry,
            &["endpoint", "fence"],
            "inventory_expected_revision",
        )?;
        let endpoint = match entry.raw_get::<Value>("endpoint")? {
            Value::Table(table) => table,
            _ => return Err(lua_input_error("inventory_endpoint", "type")),
        };
        let fence = match entry.raw_get::<Value>("fence")? {
            Value::Table(table) => table,
            _ => return Err(lua_input_error("inventory_fence", "type")),
        };
        revisions.push(ScriptInventoryExpectedRevision::new(
            parse_inventory_endpoint(&endpoint)?,
            parse_inventory_fence(&fence)?,
        ));
    }
    Ok(revisions)
}

fn parse_resource_plan(table: &Table) -> mlua::Result<ScriptInventoryResourcePlan> {
    validate_record_shape(table, &["portions"], "inventory_resource_plan")?;
    let portions = match table.raw_get::<Value>("portions")? {
        Value::Table(table) => table,
        _ => return Err(lua_input_error("inventory_portions", "type")),
    };
    let len =
        validate_sequence_shape(&portions, MAX_INVENTORY_WORK_PORTIONS, "inventory_portions")?;
    let mut parsed = Vec::with_capacity(len);
    for index in 1..=len {
        let portion = raw_table_entry(&portions, index, "inventory_portion")?;
        validate_record_shape(&portion, &["work_units", "materials"], "inventory_portion")?;
        let materials = match portion.raw_get::<Value>("materials")? {
            Value::Table(table) => table,
            _ => return Err(lua_input_error("inventory_materials", "type")),
        };
        let material_len = validate_sequence_shape(
            &materials,
            MAX_INVENTORY_RESOURCE_TYPES,
            "inventory_materials",
        )?;
        let mut parsed_materials = Vec::with_capacity(material_len);
        for material_index in 1..=material_len {
            let material = raw_table_entry(&materials, material_index, "inventory_material")?;
            validate_record_shape(&material, &["resource", "quantity"], "inventory_material")?;
            parsed_materials.push(ScriptInventoryMaterial::new(
                raw_bounded_string_field(
                    &material,
                    "resource",
                    "inventory_resource",
                    MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?,
                raw_u64_field(&material, "quantity", "inventory_quantity")?,
            ));
        }
        parsed.push(ScriptInventoryWorkPortion::new(
            raw_u64_field(&portion, "work_units", "inventory_work_units")?,
            parsed_materials,
        ));
    }
    Ok(ScriptInventoryResourcePlan::new(parsed))
}
