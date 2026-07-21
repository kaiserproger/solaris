use std::num::NonZeroUsize;

use super::*;
use crate::{
    RuntimeControls, SCRIPT_API_VERSION, ScriptCommand, ScriptEvent, ScriptInventoryResourceDelta,
    ScriptPlayerId, ScriptPlayerInventoryFailure, ScriptPlayerInventoryTransaction,
    ScriptPluginManifest,
};

fn context<'a>(controls: &'a RuntimeControls) -> RuntimeContext<'a> {
    RuntimeContext::new(controls, NonZeroUsize::new(4).unwrap())
}

#[test]
fn lua_player_inventory_call_and_targeted_result_use_exact_fields() {
    let manifest = ScriptPluginManifest::new("kits", "Kits", "0.1.0", SCRIPT_API_VERSION)
        .subscribe_event("server.started")
        .declare_player_inventory()
        .validate()
        .unwrap();
    let mut runtime = LuaScriptRuntime::from_source(
        manifest,
        r#"
            function on_server_started(_event)
                solaris.inventory_transaction(7, "grant", {
                    { resource = "minecraft:emerald", delta = -3 },
                    { resource = "minecraft:apple", delta = 2 },
                })
            end

            function on_player_inventory_transaction_result(event)
                solaris.broadcast(
                    event.request_id .. ":" .. tostring(event.player_id) .. ":"
                        .. tostring(event.committed) .. ":" .. tostring(event.failure)
                )
            end
        "#,
        LuaRuntimeLimits::default(),
    )
    .unwrap();
    let controls = RuntimeControls::unrestricted();

    let batch = runtime
        .handle_event(&ScriptEvent::server_started(), context(&controls))
        .unwrap();
    let transaction = ScriptPlayerInventoryTransaction::try_new(
        "grant",
        ScriptPlayerId::new(7),
        vec![
            ScriptInventoryResourceDelta::try_new("minecraft:emerald", -3).unwrap(),
            ScriptInventoryResourceDelta::try_new("minecraft:apple", 2).unwrap(),
        ],
    )
    .unwrap();
    assert_eq!(
        batch.commands(),
        &[ScriptCommand::PlayerInventoryTransaction {
            transaction: transaction.clone(),
        }]
    );

    let event = ScriptEvent::player_inventory_transaction_result(
        "kits",
        &transaction,
        Some(ScriptPlayerInventoryFailure::InsufficientResource),
    )
    .unwrap();
    let batch = runtime.handle_event(&event, context(&controls)).unwrap();
    assert_eq!(
        batch.commands(),
        &[ScriptCommand::BroadcastChatMessage {
            message: "grant:7:false:insufficient_resource".to_owned(),
        }]
    );
}

#[test]
fn lua_player_inventory_call_requires_declared_capability() {
    let manifest = ScriptPluginManifest::new("kits", "Kits", "0.1.0", SCRIPT_API_VERSION)
        .subscribe_event("server.started")
        .validate()
        .unwrap();
    let mut runtime = LuaScriptRuntime::from_source(
        manifest,
        r#"
            function on_server_started(_event)
                solaris.inventory_transaction(7, "grant", {
                    { resource = "minecraft:apple", delta = 1 },
                })
            end
        "#,
        LuaRuntimeLimits::default(),
    )
    .unwrap();
    let controls = RuntimeControls::unrestricted();

    let error = runtime
        .handle_event(&ScriptEvent::server_started(), context(&controls))
        .unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::Trap { message }
            if message.contains("command capability denied: player_inventory")
    ));
}
