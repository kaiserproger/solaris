use std::sync::{Arc, Mutex};

use mlua::serde::SerializeOptions;
use mlua::{Lua, LuaSerdeExt, LuaString, Table, Value};

use super::{
    DiskManifest, InvocationState, bounded_lua_string, bounded_script_id, dto_error,
    lua_input_error, parse_storage_mutations, push_command,
};
use crate::{
    MAX_PLUGIN_STORAGE_KEY_BYTES, MAX_SCRIPT_ID_BYTES, MAX_STORAGE_SCAN_PAGE, ScriptCommand,
    ScriptOperation, ScriptOperationOutcome, ScriptOperationRequest,
};

pub(super) fn validate_required_features(disk: &DiskManifest) -> Result<(), String> {
    if disk.required_features.len() > crate::MAX_MANIFEST_CAPABILITIES {
        return Err("too many required plugin features".to_owned());
    }
    for feature in &disk.required_features {
        crate::validate_script_id_value(feature).map_err(|error| error.to_string())?;
        if feature != "storage_batches" {
            return Err(format!("unsupported required plugin feature {feature:?}"));
        }
    }
    if disk
        .capabilities
        .iter()
        .any(|capability| capability == "storage_batches")
        && !disk
            .required_features
            .iter()
            .any(|feature| feature == "storage_batches")
    {
        return Err(
            "storage_batches capability requires required_features = [\"storage_batches\"]"
                .to_owned(),
        );
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
    )
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
