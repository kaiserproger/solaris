use std::fs;
use std::path::Path;

use serde_json::Value;

#[test]
fn m78_real_client_scenario_names_required_evidence() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tools/client-automation/scenarios/m78_smoke.json");
    let manifest = fs::read_to_string(&path).expect("read M78 real-client scenario manifest");
    let manifest: Value =
        serde_json::from_str(&manifest).expect("parse M78 scenario manifest JSON");

    assert_eq!(
        manifest.get("schema").and_then(Value::as_str),
        Some("solaris.real_client_scenario.v1"),
        "M78 scenario schema drifted"
    );
    assert!(
        manifest.pointer("/client/command_env").is_none(),
        "M78 scenario must not expose a free-form real-client command env hook"
    );
    assert_eq!(
        manifest.pointer("/client/adapter").and_then(Value::as_str),
        Some(
            "client-mod/solaris-client-agent/gradlew --no-configuration-cache :fabric-agent:runClientAgent"
        ),
        "M78 scenario must name the repo-native Gradle runClient adapter"
    );
    assert_eq!(
        manifest
            .pointer("/client/adapter_source")
            .and_then(Value::as_str),
        Some("auto-gradle-runclient"),
        "M78 scenario must use the auto-selected Gradle runClient adapter source"
    );

    assert_str_contains(
        &manifest,
        "/preconditions/world_prep",
        ".analysis/test-world",
        "M78 scenario must name the prepared world source",
    );
    assert_str_contains(
        &manifest,
        "/preconditions/world_prep",
        "(0, 64, 1)",
        "M78 scenario must name the block-edit target coordinates",
    );
    assert_str_contains(
        &manifest,
        "/preconditions/world_prep",
        "(2, 64, 0)",
        "M78 scenario must name the prepared container coordinates",
    );
    assert_eq!(
        manifest
            .pointer("/preconditions/gamemode")
            .and_then(Value::as_str),
        Some("survival"),
        "M78 scenario must require survival gamemode"
    );
    assert_eq!(
        manifest
            .pointer("/preconditions/hotbar_block/selected_slot")
            .and_then(Value::as_u64),
        Some(0),
        "M78 scenario must pin the selected hotbar slot"
    );
    assert_eq!(
        manifest
            .pointer("/preconditions/hotbar_block/item")
            .and_then(Value::as_str),
        Some("minecraft:dirt"),
        "M78 scenario must name the placeable hotbar block"
    );
    assert_eq!(
        manifest
            .pointer("/preconditions/hotbar_block/minimum_count")
            .and_then(Value::as_u64),
        Some(1),
        "M78 scenario must require at least one hotbar block"
    );
    assert_eq!(
        manifest
            .pointer("/preconditions/simple_container/type")
            .and_then(Value::as_str),
        Some("minecraft:chest"),
        "M78 scenario must name the simple container type"
    );
    assert_array_eq(
        &manifest,
        "/preconditions/block_edit_target/break_position",
        &[0, 64, 1],
        "M78 scenario must pin the break target coordinates",
    );
    assert_array_eq(
        &manifest,
        "/preconditions/block_edit_target/place_position",
        &[0, 64, 1],
        "M78 scenario must pin the place target coordinates",
    );
    assert_array_eq(
        &manifest,
        "/preconditions/simple_container/position",
        &[2, 64, 0],
        "M78 scenario must pin the container coordinates",
    );

    let required_files = manifest
        .pointer("/evidence/required_files")
        .and_then(Value::as_array)
        .expect("M78 scenario evidence.required_files must be an array");
    for required_file in [
        "client.log",
        "server.log",
        "run-manifest.txt",
        "observations.md",
        "screenshots/",
    ] {
        assert!(
            required_files
                .iter()
                .any(|value| value.as_str() == Some(required_file)),
            "M78 scenario manifest is missing required artifact {required_file}"
        );
    }

    let steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .expect("M78 scenario steps must be an array");
    for required_step in [
        "join",
        "wait_for_chunks",
        "move",
        "break_place_block",
        "open_close_container",
        "disconnect",
    ] {
        assert!(
            steps
                .iter()
                .any(|step| step.get("id").and_then(Value::as_str) == Some(required_step)),
            "M78 scenario manifest is missing required step {required_step}"
        );
    }

    let break_place_step = step_by_id(steps, "break_place_block");
    let break_place_action = break_place_step
        .get("action")
        .and_then(Value::as_str)
        .expect("break/place step must have an action");
    assert!(
        !break_place_action.contains("if available"),
        "M78 break/place step must be required, not optional"
    );
    assert!(
        break_place_step
            .get("evidence")
            .and_then(Value::as_str)
            .is_some_and(|evidence| evidence.contains("successful break and place")),
        "M78 break/place step must require successful evidence"
    );
    assert!(
        break_place_action.contains("survival mode")
            && break_place_action.contains("hotbar slot 0")
            && break_place_action.contains("minecraft:dirt")
            && break_place_action.contains("(0, 64, 1)"),
        "M78 break/place step must include gamemode, hotbar block, and target coordinates"
    );

    let container_step = step_by_id(steps, "open_close_container");
    let container_action = container_step
        .get("action")
        .and_then(Value::as_str)
        .expect("container step must have an action");
    assert!(
        !container_action.contains("if one is present") && !container_action.contains("otherwise"),
        "M78 container step must be required, not optional"
    );
    assert!(
        container_step
            .get("evidence")
            .and_then(Value::as_str)
            .is_some_and(
                |evidence| evidence.contains("successful container open and close")
                    && evidence.contains("(2, 64, 0)")
            ),
        "M78 container step must require successful coordinate-specific evidence"
    );
    assert!(
        container_action.contains("minecraft:chest") && container_action.contains("(2, 64, 0)"),
        "M78 container step must include prepared container type and coordinates"
    );

    let client_requirement = manifest
        .get("client_evidence_requirement")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        client_requirement.contains("real vanilla 26.1.2 client")
            || client_requirement.contains("PrismLauncher-launched"),
        "M78 scenario must require a real vanilla or PrismLauncher client"
    );

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let prepare_script =
        fs::read_to_string(repo_root.join("tools/prepare-real-client-scenario.sh"))
            .expect("read M78 real-client prepare script");
    let legacy_command_env = ["M78", "_CLIENT", "_COMMAND"].concat();
    let legacy_manifest_key = ["client", "_command="].concat();
    assert!(
        prepare_script.contains(":fabric-agent:runClientAgent")
            && prepare_script.contains("auto-gradle-runclient")
            && !prepare_script.contains(&legacy_command_env)
            && !prepare_script.contains(&legacy_manifest_key),
        "M78 prepare script must use the Gradle runClient adapter, not a command env hook"
    );
}

fn assert_str_contains(manifest: &Value, pointer: &str, needle: &str, message: &str) {
    let value = manifest
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(value.contains(needle), "{message}");
}

fn assert_array_eq(manifest: &Value, pointer: &str, expected: &[u64], message: &str) {
    let actual = manifest
        .pointer(pointer)
        .and_then(Value::as_array)
        .expect(message);
    assert_eq!(actual.len(), expected.len(), "{message}");
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.as_u64(), Some(*expected), "{message}");
    }
}

fn step_by_id<'a>(steps: &'a [Value], id: &str) -> &'a Value {
    steps
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("M78 scenario manifest is missing required step {id}"))
}
