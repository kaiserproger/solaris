use super::*;
use mc_server::{PluginHookFailure, PluginHookKind, PluginHookSection};
/// Build the real SDK component once per test process.
fn component_plugin_bytes() -> &'static [u8] {
    static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        let sdk = root.join("sdk/rust");
        let status = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "--manifest-path",
                sdk.join("Cargo.toml").to_str().expect("utf-8 path"),
                "--target",
                "wasm32-unknown-unknown",
                "--release",
                "-p",
                "solaris-hello-plugin",
            ])
            .status()
            .expect("the guest build starts");
        assert!(status.success(), "the component fixture must build");
        let module = std::fs::read(
            sdk.join("target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm"),
        )
        .expect("the guest module exists");
        wit_component::ComponentEncoder::default()
            .module(&module)
            .expect("the guest module carries its component types")
            .validate(true)
            .encode()
            .expect("the guest module encodes as a component")
    });
    BYTES.as_slice()
}

/// Deploy the fixture under `id`, configured with `config` as its `config.toml`.
fn deploy_component_fixture(root: &Path, id: &str, config: &str) {
    deploy_component_fixture_declaring(root, id, "", config);
}

/// The same, with `declarations` appended to the manifest verbatim.
///
/// The appended text is a manifest an operator would write - a `[worldgen]`
/// profile, a `[client]` bundle - so a case here exercises the parsing,
/// artifact verification and host startup a deployed package really does.
fn deploy_component_fixture_declaring(root: &Path, id: &str, declarations: &str, config: &str) {
    write_component_package(
        &component_package_directory(root, id),
        id,
        declarations,
        config,
        component_plugin_bytes(),
    );
}

/// Write `id`'s manifest, component and config into an existing directory.
fn write_component_package(
    directory: &Path,
    id: &str,
    declarations: &str,
    config: &str,
    artifact: &[u8],
) {
    std::fs::write(
            directory.join("plugin.toml"),
            format!(
                "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\nplayer_commands = [\"hello\"]\n{declarations}"
            ),
        )
        .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), artifact).expect("artifact");
    std::fs::write(directory.join("config.toml"), config).expect("config");
}

fn component_package_directory(root: &Path, id: &str) -> PathBuf {
    let directory = root.join(id);
    std::fs::create_dir(&directory).expect("plugin directory");
    directory
}
/// The fixture's component with one trailing custom section.
///
/// The same component, byte-for-byte different on disk: a custom section
/// carries no semantics, which is what makes it the right probe for a world
/// identity that must not depend on the bytes a deployment was built from.
fn component_plugin_bytes_with_custom_section(name: &str) -> Vec<u8> {
    let mut bytes = component_plugin_bytes().to_vec();
    let mut section = Vec::new();
    section.push(u8::try_from(name.len()).expect("a short section name"));
    section.extend_from_slice(name.as_bytes());
    section.extend_from_slice(b"stamped");
    bytes.push(0);
    bytes.push(u8::try_from(section.len()).expect("a short section"));
    bytes.extend_from_slice(&section);
    bytes
}

fn component_config(root: &Path) -> ServerConfig {
    let mut config: ServerConfig = toml::from_str(
        r#"
                [server]
                name = "Components"
                motd = "Components"

                [network]
                bind_address = "127.0.0.1"
                port = 25565

                [plugins]
                strict = true
            "#,
    )
    .unwrap();
    config.plugins.directory = Some(root.to_path_buf());
    config
}

/// The same, expecting exactly `expected` to be discovered.
fn component_config_expecting(root: &Path, expected: &[&str]) -> ServerConfig {
    let mut config = component_config(root);
    config.plugins.expected = expected.iter().map(|id| (*id).to_owned()).collect();
    config
}

/// Reload preparation never starts another host: strict/config refusals leave
/// the generation that owns the server boundary live.
#[tokio::test]
async fn component_reload_refusals_leave_the_running_host_active() {
    let root = tempfile::tempdir().expect("deployment root");
    deploy_component_fixture(root.path(), "hello", "greeting = \"Hi\"\n");
    let config = component_config_expecting(root.path(), &["hello"]);
    let component = prepare_component_deployment(&config)
        .await
        .expect("the deployment prepares")
        .expect("a configured directory prepares a deployment");
    let boundary = component
        .host
        .as_ref()
        .expect("the prepared host is live until the server takes it")
        .boundary()
        .clone();
    let config_dir = tempfile::tempdir().expect("config directory");
    let config_path = config_dir.path().join("config.toml");
    let write_config = |strict: bool| {
        std::fs::write(
            &config_path,
            format!(
                r#"
                    [server]
                    name = "Components"
                    motd = "Components"

                    [network]
                    bind_address = "127.0.0.1"
                    port = 25565

                    [plugins]
                    strict = {strict}
                    directory = "{}"
                    expected = ["hello"]
                "#,
                root.path().display()
            ),
        )
        .expect("reload config");
    };

    let error = prepare_configured_component_reload(&config_path, false)
        .expect_err("a permissive startup cannot reload components");
    assert!(
        error
            .to_string()
            .contains("start with plugins.strict = true"),
        "unexpected startup strict refusal: {error:#}"
    );
    boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_tick(1))
        .expect("startup strict refusal leaves the running host active");

    write_config(false);
    let error = prepare_configured_component_reload(&config_path, true)
        .expect_err("a permissive reloaded config cannot reload components");
    assert!(
        error
            .to_string()
            .contains("reloaded config must keep plugins.strict = true"),
        "unexpected config strict refusal: {error:#}"
    );
    boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_tick(2))
        .expect("config strict refusal leaves the running host active");
}

/// Reload discovery and commit reuse the boundary the server already bound; they
/// do not prepare a second host or session view.
#[tokio::test]
async fn component_reload_from_config_replaces_the_live_generation() {
    let root = tempfile::tempdir().expect("deployment root");
    deploy_component_fixture(root.path(), "hello", "greeting = \"Hi\"\n");
    let config = component_config_expecting(root.path(), &["hello"]);
    let mut component = prepare_component_deployment(&config)
        .await
        .expect("the deployment prepares")
        .expect("a configured directory prepares a deployment");
    let config_dir = tempfile::tempdir().expect("config directory");
    let config_path = config_dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
                [server]
                name = "Components"
                motd = "Components"

                [network]
                bind_address = "127.0.0.1"
                port = 25565

                [plugins]
                strict = true
                directory = "{}"
                expected = ["hello"]
            "#,
            root.path().display()
        ),
    )
    .expect("reload config");

    let report = reload_configured_component_plugins(
        config_path,
        true,
        component
            .host
            .as_ref()
            .expect("the original component host remains live"),
    )
    .await
    .expect("the equivalent component generation reloads");
    assert_eq!(report.loaded_packages, 1);
    assert_eq!(report.replaced.len(), 1);
    assert_eq!(
        component
            .host
            .as_ref()
            .expect("the reload does not replace the server-owned host")
            .boundary()
            .player_command_roots(),
        vec!["hello".to_owned()]
    );
    stop_component_host(component.take_host())
        .await
        .expect("the host stops");
}

#[tokio::test]
async fn two_component_packages_declaring_rules_refuse_the_deployment() {
    let root = tempfile::tempdir().expect("deployment root");
    deploy_component_fixture(root.path(), "first", "mode = \"placement\"\n");
    deploy_component_fixture(root.path(), "second", "mode = \"placement\"\n");
    let config = component_config_expecting(root.path(), &["first", "second"]);

    let error = prepare_component_deployment(&config)
        .await
        .err()
        .expect("two declarers must refuse the deployment");
    let message = format!("{error:#}");
    assert!(
        message.contains("first") && message.contains("second"),
        "the refusal names both declaring packages: {message}"
    );
}
/// The records a component deployment declares are the world contract, and the
/// same effective plan keeps opening the same world however the component's
/// bytes change.
#[tokio::test]
async fn component_worldgen_records_open_the_same_world_however_the_component_changes() {
    let world = tempfile::tempdir().expect("world root");
    let geometry = mc_world::OVERWORLD_GEOMETRY;
    let spawn = mc_world::WorldSpawn::new(96, -32);
    let declared_profile = "\n[worldgen]\nore_profile = \"realistic_deposits\"\n";

    let deployed = tempfile::tempdir().expect("deployment root");
    deploy_component_fixture_declaring(
        deployed.path(),
        "hello",
        declared_profile,
        "mode = \"placement\"\n",
    );
    let config = component_config_expecting(deployed.path(), &["hello"]);
    let mut component = prepare_component_deployment(&config)
        .await
        .expect("the deployment prepares")
        .expect("a configured directory prepares a deployment");
    let records = DeploymentRecords::of(Some(&component));
    let identities = records.world_contract_identities(mc_server::SettlementProfile::Vanilla);
    assert!(
        identities.gameplay_rules.is_some(),
        "the fixture's plan is part of the world identity"
    );

    assert_eq!(
        ensure_world_contract_with_spawn(
            world.path(),
            geometry,
            712_816,
            "tellus_like",
            &identities.ore_profile,
            &identities.settlement_profile,
            spawn,
            identities.gameplay_rules.as_deref(),
            identities.custom_items.as_deref(),
        )
        .expect("the declared plan opens a fresh world"),
        WorldSource::SolarisGenerated,
    );
    let persisted = std::fs::read(world_contract_path(world.path())).expect("contract");
    let contract: PersistedWorldContract =
        serde_json::from_slice(&persisted).expect("contract JSON");
    assert_eq!(contract.ore_profile, "realistic_deposits");
    assert_eq!(contract.gameplay_rules, identities.gameplay_rules);
    stop_component_host(component.take_host())
        .await
        .expect("the host stops");

    // The same declarations, shipped in a component whose bytes differ: the
    // world is unchanged, because the plan - not the artifact - is identity.
    let redeployed = tempfile::tempdir().expect("redeployment root");
    write_component_package(
        &component_package_directory(redeployed.path(), "hello"),
        "hello",
        declared_profile,
        "mode = \"placement\"\n",
        &component_plugin_bytes_with_custom_section("solaris-p4"),
    );
    let config = component_config_expecting(redeployed.path(), &["hello"]);
    let mut component = prepare_component_deployment(&config)
        .await
        .expect("the redeployment prepares")
        .expect("a configured directory prepares a deployment");
    let records = DeploymentRecords::of(Some(&component));
    let redeployed_identities =
        records.world_contract_identities(mc_server::SettlementProfile::Vanilla);
    assert_eq!(
        ensure_world_contract_with_spawn(
            world.path(),
            geometry,
            712_816,
            "tellus_like",
            &redeployed_identities.ore_profile,
            &redeployed_identities.settlement_profile,
            spawn,
            redeployed_identities.gameplay_rules.as_deref(),
            redeployed_identities.custom_items.as_deref(),
        )
        .expect("the same plan reopens the same world"),
        WorldSource::SolarisGenerated,
    );
    assert_eq!(
        std::fs::read(world_contract_path(world.path())).expect("contract"),
        persisted,
        "an equivalent deployment rewrites nothing but the same contract"
    );
    stop_component_host(component.take_host())
        .await
        .expect("the host stops");

    // A component that declares no ore profile is a different plan: refused
    // before the world is touched, leaving the contract byte-for-byte intact.
    let plain = tempfile::tempdir().expect("undeclaring deployment root");
    deploy_component_fixture(plain.path(), "hello", "mode = \"placement\"\n");
    let config = component_config_expecting(plain.path(), &["hello"]);
    let mut component = prepare_component_deployment(&config)
        .await
        .expect("the deployment prepares")
        .expect("a configured directory prepares a deployment");
    let records = DeploymentRecords::of(Some(&component));
    let undeclared = records.world_contract_identities(mc_server::SettlementProfile::Vanilla);
    ensure_world_contract_with_spawn(
        world.path(),
        geometry,
        712_816,
        "tellus_like",
        &undeclared.ore_profile,
        &undeclared.settlement_profile,
        spawn,
        undeclared.gameplay_rules.as_deref(),
        undeclared.custom_items.as_deref(),
    )
    .expect_err("a different ore profile must not open the same world");
    assert_eq!(
        std::fs::read(world_contract_path(world.path())).expect("contract"),
        persisted,
        "a refused plan leaves the persisted contract untouched"
    );
    stop_component_host(component.take_host())
        .await
        .expect("the host stops");
}

/// A startup that fails after the host started stops the host it prepared.
#[tokio::test]
async fn a_failed_startup_stops_the_prepared_component_host() {
    let root = tempfile::tempdir().expect("deployment root");
    deploy_component_fixture(root.path(), "hello", "greeting = \"Hi\"\n");
    let config = component_config_expecting(root.path(), &["hello"]);

    let component = prepare_component_deployment(&config)
        .await
        .expect("the deployment prepares")
        .expect("a configured directory prepares a deployment");
    let boundary = component
        .host
        .as_ref()
        .expect("the prepared host is live until the server takes it")
        .boundary()
        .clone();
    boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_tick(1))
        .expect("the started host admits events");

    // Every failure between here and the bind - unreadable content, a
    // contracted world the plan does not match, an unbindable listener - drops
    // the prepared deployment on its way out. That drop is the stop.
    drop(component);

    assert_eq!(
        boundary.try_enqueue_event(mc_script::ScriptEvent::server_tick(2)),
        Err(mc_script::ScriptQueueError::Closed),
        "a failed startup stops the host instead of leaking its event queue"
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("a stopped host closes its command queue")
            .is_none(),
        "a stopped host answers no commands"
    );
}

/// One `[[plugins.hooks]]` entry, as an operator writes it: the middle of the
/// chain and the default deny-on-failure policy.
fn hook_registration(plugin_id: &str, kind: PluginHookKind) -> PluginHookSection {
    PluginHookSection {
        plugin_id: plugin_id.to_owned(),
        kind,
        order: 0,
        on_failure: PluginHookFailure::Deny,
    }
}

/// A registration authorizes a hook on a deployed package, so it needs the
/// deployment: without one there is no roster to attach the handler to.
#[test]
fn configured_hooks_without_a_component_deployment_are_refused() {
    let world = tempfile::tempdir().expect("world root");
    let deployment = tempfile::tempdir().expect("deployment root");
    let mut config = component_config(deployment.path());
    config.plugins.directory = None;
    config.data.world_dir = Some(world.path().to_path_buf());
    config.plugins.hooks = vec![hook_registration("hello", PluginHookKind::BeforeDamage)];

    let error = validate_runtime_config(&config).unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("[plugins.hooks]") && message.contains("plugins.directory"),
        "the refusal names the hook surface and the missing deployment: {message}"
    );
}

/// The operator spelling is the contract: `before-build`/`before-damage` with an
/// integer order and a deny-or-keep failure policy that denies by default, and
/// anything else is a configuration error rather than a hook that quietly
/// never runs.
#[test]
fn hook_registrations_take_the_documented_spellings_and_default_to_deny() {
    let config: ServerConfig = toml::from_str(
        r#"
            [server]
            name = "Hooks"
            motd = "Hooks"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [plugins]

            [[plugins.hooks]]
            plugin_id = "first"
            kind = "before-build"
            order = -1

            [[plugins.hooks]]
            plugin_id = "second"
            kind = "before-damage"
            on_failure = "keep"
        "#,
    )
    .expect("the documented spellings parse");
    assert_eq!(config.plugins.hooks[0].order, -1);
    assert_eq!(
        config.plugins.hooks[0].on_failure,
        PluginHookFailure::Deny,
        "an unstated failure policy denies"
    );
    assert_eq!(config.plugins.hooks[1].on_failure, PluginHookFailure::Keep);

    for misspelled in [
        "plugin_id = \"first\"\nkind = \"build\"",
        "plugin_id = \"first\"\nkind = \"before-build\"\non_failure = \"allow\"",
        "plugin_id = \"first\"\nkind = \"before-build\"\noder = 1",
        "plugin_id = \"first\"\nkind = \"before-build\"\norder = \"first\"",
    ] {
        let source = format!(
            "[server]\nname = \"Hooks\"\nmotd = \"Hooks\"\n\n\
             [network]\nbind_address = \"127.0.0.1\"\nport = 25565\n\n\
             [[plugins.hooks]]\n{misspelled}\n"
        );
        assert!(
            toml::from_str::<ServerConfig>(&source).is_err(),
            "`{misspelled}` must not parse as a registration"
        );
    }
}

/// A registration names a plugin the deployment must actually have: an id no
/// package claims has no handler, so the deployment refuses instead of starting
/// with a hook the operator thought was live.
#[tokio::test]
async fn a_registration_for_an_unknown_plugin_refuses_the_deployment() {
    let root = tempfile::tempdir().expect("deployment root");
    let mut config = component_config(root.path());
    config.plugins.hooks = vec![hook_registration("ghost", PluginHookKind::BeforeBuild)];

    let error = prepare_component_deployment(&config)
        .await
        .err()
        .expect("a registration no deployed package can answer refuses the deployment");
    let message = format!("{error:#}");
    assert!(
        message.contains("ghost"),
        "the refusal names the plugin: {message}"
    );
}

/// `--check` validates the same roster startup does, before it reports anything:
/// the check of one deployment must not pass a registration serve would refuse.
#[test]
fn check_refuses_a_registration_the_deployment_cannot_authorize() {
    let plugins = tempfile::tempdir().expect("deployment root");
    let world = tempfile::tempdir().expect("world root");
    let config_dir = tempfile::tempdir().expect("config directory");
    let config_path = config_dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
                [server]
                name = "Hooks"
                motd = "Hooks"

                [network]
                bind_address = "127.0.0.1"
                port = 25565

                [data]
                world_dir = "{world}"

                [plugins]
                strict = true
                directory = "{plugins}"

                [[plugins.hooks]]
                plugin_id = "ghost"
                kind = "before-build"
            "#,
            world = world.path().display(),
            plugins = plugins.path().display(),
        ),
    )
    .expect("config");

    let error = check_config(&config_path).expect_err("--check refuses the roster serve refuses");
    let message = format!("{error:#}");
    assert!(
        message.contains("ghost"),
        "the check names the plugin: {message}"
    );
}

/// Declaring a hook in a manifest makes it available; only a registration makes
/// it run. A registration the deployed package never declared therefore has no
/// handler to authorize, and the deployment refuses rather than starting a
/// boundary that answers for a hook the package never offered.
#[tokio::test]
async fn a_registration_the_deployed_package_does_not_declare_refuses_the_deployment() {
    let root = tempfile::tempdir().expect("deployment root");
    deploy_component_fixture(root.path(), "hello", "");
    let mut config = component_config_expecting(root.path(), &["hello"]);
    config.plugins.hooks = vec![hook_registration("hello", PluginHookKind::BeforeBuild)];

    let error = prepare_component_deployment(&config)
        .await
        .err()
        .expect("the fixture package declares no precommit hook");
    let message = format!("{error:#}");
    assert!(
        message.contains("hello"),
        "the refusal names the package the registration could not be attached to: {message}"
    );
}
