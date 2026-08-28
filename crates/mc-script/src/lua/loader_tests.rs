use super::*;
use crate::{PlayerCommandAdmission, ScriptPlayerContext};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn plugin_root(test: &str) -> PathBuf {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("solaris-loader-{test}-{}-{id}", std::process::id()))
}

fn write_plugin(root: &Path, client: &str) {
    let directory = root.join("loader-test");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("plugin.toml"),
        format!(
            r#"
id = "loader-test"
name = "Loader Test"
version = "0.1.0"
api = "0.6.0"

{client}
"#
        ),
    )
    .unwrap();
    fs::write(directory.join("main.lua"), "").unwrap();
}

fn write_artifact(root: &Path, path: &str, bytes: &[u8]) {
    let artifact = root.join("loader-test").join(path);
    fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    fs::write(artifact, bytes).unwrap();
}

#[test]
fn client_bundle_manifest_covers_all_loader_content_and_cache_fences() {
    let root = plugin_root("valid");
    write_plugin(
        &root,
        r#"
[client]
schema = 1

[[client.bundles]]
id = "rich-content"
version = "1.2.3"
artifact = "client/rich-content.zip"
sha256 = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
size_bytes = 1
loaders = ["fabric", "neoforge", "forge"]
content = ["blocks", "items", "screens", "assets", "interactions"]
permissions = [
  "register_blocks",
  "register_items",
  "open_screens",
  "load_assets",
  "send_interactions",
]
"#,
    );
    write_artifact(&root, "client/rich-content.zip", b"x");

    let prepared = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap();
    let bundle = &prepared.client_bundles()[0];
    let plugin = prepared.discovered_plugins().next().unwrap();

    assert_eq!(bundle.owner_plugin_id(), "loader-test");
    assert_eq!(plugin.id(), "loader-test");
    assert_eq!(plugin.deployment(), LuaPluginDeployment::ServerAndClient);
    assert_eq!(plugin.supported_loaders(), &["fabric", "forge", "neoforge"]);
    assert_eq!(
        plugin.permissions(),
        &[
            "load_assets",
            "open_screens",
            "register_blocks",
            "register_items",
            "send_interactions"
        ]
    );
    assert_eq!(plugin.total_artifact_bytes(), 1);
    assert_eq!(plugin.client_bundles().len(), 1);
    assert_eq!(plugin.client_bundles()[0].id(), "rich-content");
    assert_eq!(plugin.client_bundles()[0].version(), "1.2.3");
    assert_eq!(
        plugin.client_bundles()[0].artifact(),
        "client/rich-content.zip"
    );
    assert_eq!(plugin.client_bundles()[0].size_bytes(), 1);
    assert_eq!(
        plugin.client_bundles()[0].loaders(),
        &["fabric", "neoforge", "forge"]
    );
    assert_eq!(
        plugin.client_bundles()[0].content(),
        &["blocks", "items", "screens", "assets", "interactions"]
    );
    assert_eq!(bundle.id(), "rich-content");
    assert_eq!(
        bundle.loaders(),
        &[
            LuaClientLoader::Fabric,
            LuaClientLoader::NeoForge,
            LuaClientLoader::Forge
        ]
    );
    assert_eq!(
        bundle.content(),
        &[
            LuaClientContentKind::Blocks,
            LuaClientContentKind::Items,
            LuaClientContentKind::Screens,
            LuaClientContentKind::Assets,
            LuaClientContentKind::Interactions,
        ]
    );
    assert_eq!(
        bundle.cache_key(),
        "loader-test:rich-content/1.2.3/2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
    );
    assert_eq!(
        bundle.artifact_path(),
        fs::canonicalize(root.join("loader-test/client/rich-content.zip")).unwrap()
    );
    assert_eq!(bundle.artifact_bytes(), b"x");
    fs::write(root.join("loader-test/client/rich-content.zip"), b"y").unwrap();
    assert_eq!(bundle.artifact_bytes(), b"x");
    fs::remove_file(root.join("loader-test/client/rich-content.zip")).unwrap();
    assert_eq!(bundle.artifact_bytes(), b"x");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn plugin_without_client_bundles_is_discovered_as_server_only() {
    let root = plugin_root("server-only");
    write_plugin(&root, "");

    let prepared = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap();
    let plugins = prepared.discovered_plugins().collect::<Vec<_>>();

    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].id(), "loader-test");
    assert_eq!(plugins[0].deployment(), LuaPluginDeployment::ServerOnly);
    assert!(plugins[0].supported_loaders().is_empty());
    assert!(plugins[0].permissions().is_empty());
    assert_eq!(plugins[0].total_artifact_bytes(), 0);
    assert!(plugins[0].client_bundles().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn client_bundle_rejects_artifact_bytes_that_do_not_match_the_manifest() {
    let root = plugin_root("artifact-hash");
    write_plugin(
        &root,
        r#"
[client]
schema = 1

[[client.bundles]]
id = "assets"
version = "1"
artifact = "client/assets.zip"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
size_bytes = 1
loaders = ["fabric"]
content = ["assets"]
permissions = ["load_assets"]
"#,
    );
    write_artifact(&root, "client/assets.zip", b"x");

    let error = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap_err();

    assert!(matches!(
        error,
        LuaHostError::InvalidStartupPlugin { message, .. }
            if message.contains("SHA-256 does not match")
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn client_bundle_missing_content_permission_fails_startup() {
    let root = plugin_root("permission");
    write_plugin(
        &root,
        r#"
[client]
schema = 1

[[client.bundles]]
id = "screen"
version = "1"
artifact = "client/screen.zip"
sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
size_bytes = 32
loaders = ["fabric"]
content = ["screens"]
permissions = ["load_assets"]
"#,
    );

    let error = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap_err();

    assert!(matches!(
        error,
        LuaHostError::InvalidStartupPlugin { message, .. }
            if message.contains("requires permission")
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn client_bundle_rejects_traversal_and_noncanonical_hashes() {
    let root = plugin_root("path");
    write_plugin(
        &root,
        r#"
[client]
schema = 1

[[client.bundles]]
id = "assets"
version = "1"
artifact = "../assets.zip"
sha256 = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
size_bytes = 32
loaders = ["forge"]
content = ["assets"]
permissions = ["load_assets"]
"#,
    );

    let error = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap_err();

    assert!(matches!(
        error,
        LuaHostError::InvalidStartupPlugin { message, .. }
            if message.contains("artifact path")
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn client_bundle_rejects_cache_path_segments() {
    let root = plugin_root("cache-path");
    write_plugin(
        &root,
        r#"
[client]
schema = 1

[[client.bundles]]
id = "assets"
version = ".."
artifact = "client/assets.zip"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
size_bytes = 32
loaders = ["forge"]
content = ["assets"]
permissions = ["load_assets"]
"#,
    );

    let error = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap_err();

    assert!(matches!(
        error,
        LuaHostError::InvalidStartupPlugin { message, .. }
            if message.contains("relative path segment")
    ));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn shipped_two_owner_live_gate_fixture_is_discoverable_and_runnable() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/loader-live-gate/plugins");
    let prepared = prepare_lua_plugins(LuaHostConfig::new(&fixture)).unwrap();
    let bundles = prepared
        .client_bundles()
        .iter()
        .map(|bundle| (bundle.owner_plugin_id(), bundle))
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_eq!(bundles.len(), 2);
    for owner in ["ruby-live", "sapphire-live"] {
        let bundle = bundles.get(owner).unwrap();
        assert_eq!(bundle.id(), "rich-content");
        assert_eq!(bundle.version(), "1");
        assert_eq!(bundle.loaders().len(), 3);
        assert_eq!(bundle.content().len(), 5);
        assert_eq!(bundle.permissions().len(), 5);
        assert!(bundle.artifact_path().is_file());
    }

    let (boundary, host) = start_lua_host(LuaHostConfig::new(fixture)).unwrap();
    assert_eq!(host.loaded_plugins(), 2);
    assert_eq!(
        boundary.player_command_roots(),
        vec!["loader_ruby".to_owned(), "loader_sapphire".to_owned()]
    );
    let player_id = ScriptPlayerId::new(7);
    let player_context =
        ScriptPlayerContext::new("fixture-player", "SolarisLoader", false, 0.0, 64.0, 0.0);
    for (owner, command, block_id, screen_id) in [
        (
            "ruby-live",
            "loader_ruby",
            "ruby-live:ruby_block",
            "ruby-live:showcase",
        ),
        (
            "sapphire-live",
            "loader_sapphire",
            "sapphire-live:sapphire_block",
            "sapphire-live:showcase",
        ),
    ] {
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                player_id,
                player_context.clone(),
                command,
            ),
            Ok(PlayerCommandAdmission::Enqueued)
        );
        let grant = boundary.recv_command().await.unwrap();
        assert!(matches!(
            grant,
            ScriptCommand::HostAttached { provenance, request }
                if provenance.plugin_id() == owner
                    && matches!(
                        request.as_ref(),
                        ScriptCommand::GrantLoaderBlockItem { request }
                            if request.player_id() == player_id
                                && request.block_id() == block_id
                                && request.count() == 1
                    )
        ));
        let screen = boundary.recv_command().await.unwrap();
        assert!(matches!(
            screen,
            ScriptCommand::HostAttached { provenance, request }
                if provenance.plugin_id() == owner
                    && matches!(
                        request.as_ref(),
                        ScriptCommand::OpenClientScreen {
                            player_id: target,
                            screen_id: requested_screen,
                        } if *target == player_id && requested_screen == screen_id
                    )
        ));
    }

    for (owner, interaction_id, payload, expected_message) in [
        (
            "ruby-live",
            "ruby-live:confirm",
            "ruby-confirmed",
            "Ruby Loader interaction reached ruby-live.",
        ),
        (
            "sapphire-live",
            "sapphire-live:confirm",
            "sapphire-confirmed",
            "Sapphire Loader interaction reached sapphire-live.",
        ),
    ] {
        boundary
            .enqueue_targeted_event(
                ScriptEvent::loader_interaction(owner, player_id, interaction_id, payload).unwrap(),
            )
            .await
            .unwrap();
        let response = boundary.recv_command().await.unwrap();
        assert!(matches!(
            response,
            ScriptCommand::HostAttached { provenance, request }
                if provenance.plugin_id() == owner
                    && matches!(
                        request.as_ref(),
                        ScriptCommand::SendChatMessage {
                            player_id: target,
                            message,
                        } if *target == player_id && message == expected_message
                    )
        ));
    }
    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

fn write_graph_plugin(root: &Path, directory: &str, id: &str, extra_manifest: &str) {
    let plugin_dir = root.join(directory);
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("plugin.toml"),
        format!(
            r#"
id = "{id}"
name = "{id}"
version = "0.1.0"
api = "0.6.0"
{extra_manifest}
"#
        ),
    )
    .unwrap();
    fs::write(plugin_dir.join("main.lua"), "").unwrap();
}

#[test]
fn dependency_graph_rejects_missing_required_plugin() {
    let root = plugin_root("missing-required");
    write_graph_plugin(
        &root,
        "dependent",
        "dependent",
        r#"
[[dependencies]]
id = "missing"
relation = "required"
"#,
    );

    let error = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap_err();
    assert!(matches!(
        error,
        LuaHostError::DependencyGraph { message }
            if message.contains("dependent") && message.contains("missing")
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dependency_graph_orders_required_optional_and_load_before_deterministically() {
    let root = plugin_root("topological-order");
    write_graph_plugin(
        &root,
        "00-dependent",
        "dependent",
        r#"
[[dependencies]]
id = "base"
relation = "required"
"#,
    );
    write_graph_plugin(
        &root,
        "10-optional",
        "optional",
        r#"
[[dependencies]]
id = "absent"
relation = "optional"
"#,
    );
    write_graph_plugin(&root, "20-target", "target", "");
    write_graph_plugin(
        &root,
        "30-before",
        "before",
        r#"
[[dependencies]]
id = "target"
relation = "load_before"
"#,
    );
    write_graph_plugin(&root, "99-base", "base", "");

    let prepared = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap();
    let order = prepared
        .discovered_plugins()
        .map(|plugin| plugin.id().to_owned())
        .collect::<Vec<_>>();
    assert!(
        order.iter().position(|id| id == "base").unwrap()
            < order.iter().position(|id| id == "dependent").unwrap()
    );
    assert!(
        order.iter().position(|id| id == "before").unwrap()
            < order.iter().position(|id| id == "target").unwrap()
    );
    assert!(order.contains(&"optional".to_owned()));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dependency_graph_orders_startup_before_post_world() {
    let root = plugin_root("phase-order");
    write_graph_plugin(&root, "00-post", "post", "");
    write_graph_plugin(&root, "99-start", "start", "load_phase = \"startup\"");

    let prepared = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap();
    let order = prepared
        .discovered_plugins()
        .map(|plugin| plugin.id().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(order, ["start", "post"]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dependency_graph_rejects_cycles_with_diagnostic_ids() {
    let root = plugin_root("cycle");
    write_graph_plugin(
        &root,
        "a",
        "a",
        r#"
[[dependencies]]
id = "b"
relation = "required"
"#,
    );
    write_graph_plugin(
        &root,
        "b",
        "b",
        r#"
[[dependencies]]
id = "a"
relation = "required"
"#,
    );

    let error = prepare_lua_plugins(LuaHostConfig::new(&root)).unwrap_err();
    assert!(matches!(
        error,
        LuaHostError::DependencyGraph { message }
            if message.contains("a") && message.contains("b") && message.contains("cycle")
    ));
    fs::remove_dir_all(root).unwrap();
}
