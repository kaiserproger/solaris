use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::{
    RuntimeControls, SCRIPT_API_VERSION, ScriptCommand, ScriptEvent, ScriptPluginManifest,
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempPluginDir(std::path::PathBuf);

impl TempPluginDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "solaris-mc-script-config-{name}-{}-{}",
            std::process::id(),
            NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempPluginDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn test_manifest() -> ValidatedScriptPluginManifest {
    ScriptPluginManifest::new("configured", "Configured", "0.1.0", SCRIPT_API_VERSION)
        .subscribe_event("server.started")
        .validate()
        .unwrap()
}

#[tokio::test]
async fn lua_plugin_reads_nested_config_as_a_fresh_table() {
    let root = TempPluginDir::new("nested");
    let plugin = root.path().join("configured");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "configured"
            name = "Configured"
            version = "0.1.0"
            api = "0.6.0"
            events = ["server.started"]
        "#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("config.toml"),
        r#"
            enabled = true
            limit = 7
            ratio = 1.5
            names = ["one", "two"]
            items = [{ resource = "minecraft:apple", count = 2 }]

            [currency]
            resource = "minecraft:gold_ingot"
        "#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            local config = solaris.config()
            assert(config.enabled == true)
            assert(config.limit == 7)
            assert(config.ratio == 1.5)
            assert(config.names[1] == "one")
            assert(config.names[2] == "two")
            assert(config.items[1].resource == "minecraft:apple")
            assert(config.items[1].count == 2)
            assert(config.currency.resource == "minecraft:gold_ingot")

            config.enabled = false
            config.names[1] = "changed"
            config.items[1].count = 99
            config.currency.resource = "minecraft:diamond"

            function on_server_started(_event)
                local fresh = solaris.config()
                assert(fresh.enabled == true)
                assert(fresh.names[1] == "one")
                assert(fresh.items[1].count == 2)
                solaris.broadcast(fresh.currency.resource)
            end
        "#,
    )
    .unwrap();

    let (boundary, host) = start_lua_host(LuaHostConfig::new(root.path())).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    std::fs::write(
        plugin.join("config.toml"),
        "[currency]\nresource = \"minecraft:diamond\"\n",
    )
    .unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();

    let command = tokio::time::timeout(std::time::Duration::from_secs(5), boundary.recv_command())
        .await
        .expect("configured plugin command")
        .unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert_eq!(
        admitted.request(),
        &ScriptCommand::BroadcastChatMessage {
            message: "minecraft:gold_ingot".to_owned(),
        }
    );

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn missing_disk_config_loads_as_an_empty_table() {
    let root = TempPluginDir::new("missing");
    let plugin = root.path().join("missing");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "missing"
            name = "Missing"
            version = "0.1.0"
            api = "0.6.0"
            events = ["server.started"]
        "#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            assert(next(solaris.config()) == nil)
            function on_server_started(_event)
                solaris.broadcast("missing-config-is-empty")
            end
        "#,
    )
    .unwrap();

    let (boundary, host) = start_lua_host(LuaHostConfig::new(root.path())).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let command = tokio::time::timeout(std::time::Duration::from_secs(5), boundary.recv_command())
        .await
        .expect("empty-config plugin command")
        .unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert!(matches!(
        admitted.request(),
        ScriptCommand::BroadcastChatMessage { message }
            if message == "missing-config-is-empty"
    ));

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn direct_runtime_without_disk_config_receives_an_empty_table() {
    let manifest = test_manifest();
    let mut runtime = LuaScriptRuntime::from_source(
        manifest,
        r#"
            local config = solaris.config()
            assert(next(config) == nil)

            function on_server_started(_event)
                solaris.broadcast("empty-config")
            end
        "#,
        LuaRuntimeLimits::default(),
    )
    .unwrap();
    let controls = RuntimeControls::unrestricted();
    let batch = runtime
        .handle_event(
            &ScriptEvent::server_started(),
            RuntimeContext::new(&controls, NonZeroUsize::new(4).unwrap()),
        )
        .unwrap();
    assert_eq!(
        batch.commands(),
        &[ScriptCommand::BroadcastChatMessage {
            message: "empty-config".to_owned(),
        }]
    );
}

#[test]
fn disk_config_rejects_values_that_cannot_cross_the_lua_boundary_safely() {
    let root = TempPluginDir::new("invalid-values");
    let cases = [
        (
            "datetime",
            "value = 1979-05-27T07:32:00Z".to_owned(),
            "datetime values are unsupported",
        ),
        (
            "non-finite",
            "value = nan".to_owned(),
            "floating-point values must be finite",
        ),
        (
            "deep",
            "[a.b.c.d.e.f.g.h.i]\nvalue = true".to_owned(),
            "nesting exceeds 8 levels",
        ),
        (
            "mixed-deep",
            "value = [{ a = [{ b = [{ c = [{ d = [{ e = true }] }] }] }] }]".to_owned(),
            "nesting exceeds 8 levels",
        ),
        (
            "long-key",
            format!("{} = true", "k".repeat(129)),
            "key exceeds 128 bytes",
        ),
        (
            "long-string",
            format!("value = {:?}", "x".repeat(4097)),
            "string exceeds 4096 bytes",
        ),
        (
            "large-array",
            format!(
                "value = [{}]",
                std::iter::repeat_n("true", 129)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            "array exceeds 128 entries",
        ),
        (
            "large-table",
            (0..129)
                .map(|index| format!("key_{index} = true"))
                .collect::<Vec<_>>()
                .join("\n"),
            "table exceeds 128 entries",
        ),
    ];

    for (name, source, expected) in cases {
        let plugin = root.path().join(name);
        std::fs::create_dir(&plugin).unwrap();
        std::fs::write(plugin.join("config.toml"), source).unwrap();
        let error = read_plugin_config(&plugin).unwrap_err();
        assert!(
            error.contains(expected),
            "{name}: expected {expected:?}, got {error:?}"
        );
    }
}

#[tokio::test]
async fn invalid_config_skips_only_its_plugin_before_command_registration() {
    let root = TempPluginDir::new("isolated-rejection");
    for (id, root_command) in [("bad", "bad-command"), ("good", "good-command")] {
        let plugin = root.path().join(id);
        std::fs::create_dir(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.toml"),
            format!(
                r#"
                    id = "{id}"
                    name = "{id}"
                    version = "0.1.0"
                    api = "0.6.0"
                    events = ["server.started"]
                    player_commands = ["{root_command}"]
                "#
            ),
        )
        .unwrap();
        std::fs::write(
            plugin.join("main.lua"),
            format!(
                r#"
                    function on_server_started(_event)
                        solaris.broadcast("{id}")
                    end
                "#
            ),
        )
        .unwrap();
    }
    std::fs::write(root.path().join("bad/config.toml"), "ignored = 1979-05-27").unwrap();

    let (boundary, host) = start_lua_host(LuaHostConfig::new(root.path())).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    assert_eq!(boundary.player_command_roots(), vec!["good-command"]);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let command = tokio::time::timeout(std::time::Duration::from_secs(5), boundary.recv_command())
        .await
        .expect("valid sibling plugin command")
        .unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert!(matches!(
        admitted.request(),
        ScriptCommand::BroadcastChatMessage { message } if message == "good"
    ));

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn disk_config_file_size_is_bounded_before_parsing() {
    let root = TempPluginDir::new("file-size");
    let exact = root.path().join("exact");
    std::fs::create_dir(&exact).unwrap();
    let prefix = "value = true\n#";
    std::fs::write(
        exact.join("config.toml"),
        format!(
            "{prefix}{}",
            "x".repeat(MAX_PLUGIN_CONFIG_BYTES - prefix.len())
        ),
    )
    .unwrap();
    read_plugin_config(&exact).unwrap();

    let plugin = root.path().join("oversized");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("config.toml"),
        "x".repeat(MAX_PLUGIN_CONFIG_BYTES + 1),
    )
    .unwrap();

    let error = read_plugin_config(&plugin).unwrap_err();
    assert!(error.contains("config.toml exceeds 65536 bytes"));
}

#[test]
fn disk_config_accepts_each_exact_structural_boundary() {
    let root = TempPluginDir::new("boundaries");
    let cases = [
        ("depth", "[a.b.c.d.e.f.g.h]\nvalue = true".to_owned()),
        (
            "mixed-depth",
            "value = [{ a = [{ b = [{ c = [{ d = true }] }] }] }]".to_owned(),
        ),
        ("key", format!("{} = true", "k".repeat(128))),
        ("string", format!("value = {:?}", "x".repeat(4096))),
        (
            "array",
            format!(
                "value = [{}]",
                std::iter::repeat_n("true", 128)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
        (
            "table",
            (0..128)
                .map(|index| format!("key_{index} = true"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    ];

    for (name, source) in cases {
        let plugin = root.path().join(name);
        std::fs::create_dir(&plugin).unwrap();
        std::fs::write(plugin.join("config.toml"), source).unwrap();
        read_plugin_config(&plugin).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn basic_economy_rejects_fail_late_config_shapes_during_plugin_load() {
    let catalog = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins/basic-economy");

    let mut float_count = read_plugin_source(&catalog).unwrap();
    float_count
        .config
        .get_mut("catalog")
        .and_then(toml::Value::as_array_mut)
        .and_then(|items| items.first_mut())
        .and_then(toml::Value::as_table_mut)
        .unwrap()
        .insert("count".to_owned(), toml::Value::Float(2.5));
    let error = match LuaPlugin::new(float_count) {
        Ok(_) => panic!("floating catalog count loaded"),
        Err(error) => error,
    };
    assert!(error.contains("catalog count must be an integer"));

    let mut invalid_zone_id = read_plugin_source(&catalog).unwrap();
    invalid_zone_id
        .config
        .get_mut("zone")
        .and_then(toml::Value::as_table_mut)
        .unwrap()
        .insert(
            "id".to_owned(),
            toml::Value::String("invalid.zone".to_owned()),
        );
    let error = match LuaPlugin::new(invalid_zone_id) {
        Ok(_) => panic!("invalid zone id loaded"),
        Err(error) => error,
    };
    assert!(error.contains("zone.id contains invalid characters"));

    let mut out_of_range_zone = read_plugin_source(&catalog).unwrap();
    out_of_range_zone
        .config
        .get_mut("zone")
        .and_then(toml::Value::as_table_mut)
        .and_then(|zone| zone.get_mut("maximum"))
        .and_then(toml::Value::as_table_mut)
        .unwrap()
        .insert("x".to_owned(), toml::Value::Integer(30_000_001));
    let error = match LuaPlugin::new(out_of_range_zone) {
        Ok(_) => panic!("out-of-range zone loaded"),
        Err(error) => error,
    };
    assert!(error.contains("zone x bounds are out of range"));

    let mut exact_bounds = read_plugin_source(&catalog).unwrap();
    let zone = exact_bounds
        .config
        .get_mut("zone")
        .and_then(toml::Value::as_table_mut)
        .unwrap();
    let maximum = zone
        .get_mut("maximum")
        .and_then(toml::Value::as_table_mut)
        .unwrap();
    maximum.insert("x".to_owned(), toml::Value::Integer(30_000_000));
    maximum.insert("y".to_owned(), toml::Value::Integer(20_000_000));
    match LuaPlugin::new(exact_bounds) {
        Ok(_) => {}
        Err(error) => panic!("exact coordinate bounds rejected: {error}"),
    }

    let mut out_of_range_y = read_plugin_source(&catalog).unwrap();
    out_of_range_y
        .config
        .get_mut("zone")
        .and_then(toml::Value::as_table_mut)
        .and_then(|zone| zone.get_mut("maximum"))
        .and_then(toml::Value::as_table_mut)
        .unwrap()
        .insert("y".to_owned(), toml::Value::Integer(20_000_001));
    let error = match LuaPlugin::new(out_of_range_y) {
        Ok(_) => panic!("out-of-range vertical zone loaded"),
        Err(error) => error,
    };
    assert!(error.contains("zone y bounds are out of range"));

    let mut duplicate_product_id = read_plugin_source(&catalog).unwrap();
    let products = duplicate_product_id
        .config
        .get_mut("catalog")
        .and_then(toml::Value::as_array_mut)
        .unwrap();
    let first_id = products[0]
        .as_table()
        .and_then(|item| item.get("id"))
        .cloned()
        .unwrap();
    products[1]
        .as_table_mut()
        .unwrap()
        .insert("id".to_owned(), first_id);
    let error = match LuaPlugin::new(duplicate_product_id) {
        Ok(_) => panic!("duplicate catalog id loaded"),
        Err(error) => error,
    };
    assert!(error.contains("catalog ids must be unique"));
}
