//! What a started component deployment publishes, and what it refuses.
//!
//! These facts belong to the deployment rather than to one package: the catalog
//! core reads through the boundary, the client bundles the Loader stages, and the
//! one ore profile and settlement plan a world opens against. The host settles
//! them before the first guest runs; a duplicate worldgen profile is refused
//! because the world has no second slot for it.

mod fixture;

use fixture::component_bytes;
use mc_plugin_host::{
    CheckError, DeploymentConfig, DiscoveryMode, HostQueues, HostStartError, NoSessions,
    PluginLimits, PluginReloadContractField, PluginReloadError, check_deployment, start_deployment,
};
use mc_script::{
    PluginWorldgenOreProfile, PluginWorldgenSettlementProfile, ScriptCommand, ScriptEvent,
    ScriptPlayerContext, ScriptPlayerId,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The bytes of the one client artifact these cases deploy.
const BUNDLE_BYTES: &[u8] = b"solaris-client-fixture";

/// Its size and SHA-256, which the manifest has to declare exactly: they are the
/// bytes really written, so the package parser verifies the artifact the way it
/// does for a deployed package.
const BUNDLE_SIZE: u64 = 22;
const BUNDLE_SHA256: &str = "9ceec9527c8a41887afc870b8d120967563862cb48a2b69cf4def113e27d3ee2";

/// The client declarations one package carries.
///
/// One bundle whose content is items, with exactly the permission that content
/// needs: a declaration the parser accepts as it stands, so a case here is about
/// what the *host* publishes rather than about the parser.
fn client_declarations() -> String {
    format!(
        "\n[client]\nschema = 2\n\n[[client.bundles]]\nid = \"rich-content\"\nversion = \"1\"\n\
         artifact = \"client/rich-content.bin\"\nsha256 = \"{BUNDLE_SHA256}\"\n\
         size_bytes = {BUNDLE_SIZE}\nloaders = [\"fabric\"]\ncontent = [\"items\"]\n\
         permissions = [\"register_items\"]\n"
    )
}

/// Write one package of the fixture with `declarations` appended to its manifest.
fn write_package(root: &Path, id: &str, declarations: &str) {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n{declarations}"
        ),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
}

/// Write one package together with the artifact its client bundle declares.
fn write_package_with_bundle(root: &Path, id: &str) {
    write_package(root, id, &client_declarations());
    let artifact = root.join(id).join("client/rich-content.bin");
    std::fs::create_dir_all(artifact.parent().expect("artifact parent")).expect("artifact dir");
    std::fs::write(&artifact, BUNDLE_BYTES).expect("client artifact");
}

fn deployment(root: &Path, expected: &[&str]) -> DeploymentConfig {
    DeploymentConfig {
        root: root.to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: expected.iter().map(|id| (*id).to_owned()).collect(),
        grants: BTreeMap::new(),
        require_grants: false,
        precommit_hooks: Vec::new(),
    }
}

fn packages(root: &Path, expected: &[&str]) -> Vec<mc_plugin_host::LoadedPackage> {
    let limits = PluginLimits::default();
    mc_plugin_host::discover(&deployment(root, expected), &limits)
        .expect("deployment")
        .into_packages()
}
fn start_with_limits(
    root: &Path,
    expected: &[&str],
    limits: PluginLimits,
) -> mc_plugin_host::PluginHost {
    start_deployment(
        packages(root, expected),
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("host starts")
}

fn start(root: &Path, expected: &[&str]) -> mc_plugin_host::PluginHost {
    // These tests own reload, routing and timer semantics, not guest budgets:
    // the default watchdog (4 x 25ms) is for guests that misbehave, while a
    // loaded CI runner can stall a legitimate init past it. Widen the
    // wall-clock allowance without touching any other bound.
    start_with_limits(
        root,
        expected,
        PluginLimits {
            epoch_ticks_per_call: 64,
            ..PluginLimits::default()
        },
    )
}

fn broadcast_message(command: ScriptCommand) -> String {
    let ScriptCommand::HostAttached { request, .. } = command else {
        panic!("timer callback retains host provenance");
    };
    let ScriptCommand::BroadcastChatMessage { message } = request.as_ref() else {
        panic!("timer callback publishes its observable receipt");
    };
    message.clone()
}

#[test]
fn a_started_deployment_publishes_its_catalog_and_client_bundles() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package_with_bundle(root.path(), "packer");
    let host = start(root.path(), &["packer"]);

    // The catalog core reads: the package discovery validated, with the directory
    // its authored data lives in and the features it declared.
    let catalog = host.boundary().deployed_packages();
    assert_eq!(
        catalog.len(),
        1,
        "the deployment's own packages are the catalog"
    );
    assert_eq!(catalog[0].plugin_id(), "packer");
    assert_eq!(
        catalog[0].package_dir().to_path_buf(),
        root.path().join("packer")
    );
    assert!(catalog[0].required_features().is_empty());

    // The bundles the Loader stages: the package's own descriptor, carrying the
    // bytes and hash discovery verified on disk.
    let bundles = host.client_bundles();
    assert_eq!(bundles.len(), 1);
    let bundle = &bundles[0];
    assert_eq!(bundle.owner_plugin_id(), "packer");
    assert_eq!(bundle.id(), "rich-content");
    assert_eq!(bundle.sha256(), BUNDLE_SHA256);
    assert_eq!(bundle.size_bytes(), BUNDLE_SIZE);
    assert_eq!(bundle.artifact_bytes(), BUNDLE_BYTES);
    let canonical = std::fs::canonicalize(root.path().join("packer/client/rich-content.bin"))
        .expect("canonical artifact path");
    assert_eq!(bundle.artifact_path(), canonical.as_path());

    // A package that declares no worldgen profile contributes none.
    assert_eq!(host.worldgen_ore_profile(), None);
    assert!(host.worldgen_settlement_plan().is_none());
    host.stop();

    // The check settles the same deployment-level declarations, so a caller can
    // validate them (`LoaderManifest::from_script_bundles`) without running the
    // phases again.
    let report = check_deployment(
        &deployment(root.path(), &["packer"]),
        &PluginLimits::default(),
    )
    .expect("the deployment checks");
    let checked_bundles = report.client_bundles();
    assert_eq!(checked_bundles.len(), 1);
    assert_eq!(checked_bundles[0].owner_plugin_id(), "packer");
    assert_eq!(checked_bundles[0].id(), "rich-content");
    assert_eq!(checked_bundles[0].sha256(), BUNDLE_SHA256);
    assert_eq!(checked_bundles[0].artifact_bytes(), BUNDLE_BYTES);
}

#[tokio::test]
async fn compatible_reload_replaces_the_component_generation() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "reloadable",
        "\nplayer_commands = [\"reloadable\"]\n",
    );
    let host = start(root.path(), &["reloadable"]);
    assert_eq!(
        host.boundary().player_command_roots(),
        vec!["reloadable".to_owned()]
    );

    let report = host
        .reload(packages(root.path(), &["reloadable"]))
        .await
        .expect("an equivalent candidate reloads");
    assert_eq!(report.loaded_packages, 1);
    assert_eq!(report.replaced.len(), 1);
    assert_eq!(
        host.boundary().player_command_roots(),
        vec!["reloadable".to_owned()],
        "the replacement owns the same routed command"
    );
    host.stop();
}

/// A committed reload constructs fresh runtime stores and timer schedules. The
/// retired store's due timer cannot leak into the replacement generation.
#[tokio::test]
async fn compatible_reload_restarts_the_runtime_timer_store() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "reloadable",
        "\nevents = [\"player.joined\"]\n",
    );
    std::fs::write(
        root.path().join("reloadable/config.toml"),
        "mode = \"timers\"\nscript = \"probe\"\n",
    )
    .expect("timer fixture config");
    let host = start(root.path(), &["reloadable"]);
    let boundary = host.boundary().clone();
    let join = || {
        ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::try_new(
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "Ada",
                false,
                0.0,
                64.0,
                0.0,
            )
            .expect("valid fixture player"),
        )
    };
    boundary
        .try_enqueue_event(join())
        .expect("pre-reload join queues");

    host.reload(packages(root.path(), &["reloadable"]))
        .await
        .expect("equivalent timer deployment reloads");
    boundary
        .try_enqueue_event(join())
        .expect("replacement join queues");
    boundary
        .try_enqueue_event(ScriptEvent::server_tick(1))
        .expect("first replacement tick queues");
    let first = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("replacement timers fire")
        .expect("replacement timer publishes one command");
    let second = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("replacement timers fire")
        .expect("replacement timer publishes one command");
    assert_eq!(
        [broadcast_message(first), broadcast_message(second)],
        ["probe-init:1:1", "probe-join-1:1:1"],
        "the replacement has one fresh init timer and its global join counter starts at one"
    );

    boundary.close_event_admission();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
            .await
            .expect("retired timer store drains")
            .is_none(),
        "the retired store cannot deliver its pre-reload timer"
    );
    host.stop();
}

#[tokio::test]
async fn refused_reload_candidate_leaves_the_running_generation_active() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "reloadable",
        "\nplayer_commands = [\"reloadable\"]\n",
    );
    let host = start(root.path(), &["reloadable"]);

    std::fs::write(
        root.path().join("reloadable/config.toml"),
        "x".repeat(PluginLimits::default().text_bytes + 1),
    )
    .expect("candidate config");
    let error = host
        .reload(packages(root.path(), &["reloadable"]))
        .await
        .expect_err("a candidate configuration failure is refused before the swap");
    assert!(matches!(error, PluginReloadError::CandidateSetup { .. }));
    assert_eq!(
        host.boundary().player_command_roots(),
        vec!["reloadable".to_owned()],
        "the original component keeps its active route"
    );
    host.stop();
}

#[tokio::test]
async fn reload_candidate_memory_budget_preserves_the_running_generation() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "reloadable",
        "\nplayer_commands = [\"reloadable\"]\n",
    );
    let host = start_with_limits(
        root.path(),
        &["reloadable"],
        PluginLimits {
            reload_candidate_memory_bytes: 0,
            ..PluginLimits::default()
        },
    );

    let error = host
        .reload(packages(root.path(), &["reloadable"]))
        .await
        .expect_err("an over-budget candidate is refused before construction");
    assert!(matches!(error, PluginReloadError::CandidateMemory { .. }));
    assert_eq!(
        host.boundary().player_command_roots(),
        vec!["reloadable".to_owned()],
        "the original component keeps its active route"
    );
    host.stop();
}
#[tokio::test]
async fn restart_only_reload_contract_keeps_the_running_generation_active() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "reloadable",
        "\nplayer_commands = [\"reloadable\"]\n",
    );
    let host = start(root.path(), &["reloadable"]);
    std::fs::write(
        root.path().join("reloadable/plugin.toml"),
        "id = \"reloadable\"\nname = \"reloadable\"\nversion = \"0.2.0\"\napi = \"0.7.0\"\nplayer_commands = [\"different-root\"]\n",
    )
    .expect("candidate manifest");

    let error = host
        .reload(packages(root.path(), &["reloadable"]))
        .await
        .expect_err("a changed command owner needs a restart");
    assert!(matches!(
        error,
        PluginReloadError::RestartContractChanged {
            field: PluginReloadContractField::CommandAndChannelOwnership
        }
    ));
    assert_eq!(
        host.boundary().player_command_roots(),
        vec!["reloadable".to_owned()],
        "the original component keeps its active route"
    );
    host.stop();
}

#[test]
fn a_deployment_exposes_the_worldgen_profiles_its_packages_declare() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "ore",
        "\n[worldgen]\nore_profile = \"realistic_deposits\"\n",
    );
    write_package(
        root.path(),
        "village",
        "\n[worldgen]\nsettlement_profile = \"plains_village_prototype\"\n",
    );
    let host = start(root.path(), &["ore", "village"]);

    assert_eq!(
        host.worldgen_ore_profile(),
        Some(PluginWorldgenOreProfile::RealisticDeposits),
        "the profile the package declared"
    );
    let plan = host
        .worldgen_settlement_plan()
        .expect("the village package declared a settlement profile");
    assert_eq!(
        plan.owner_plugin_id(),
        "village",
        "the plan's owner is the package that declared it"
    );
    assert_eq!(
        plan.profile(),
        PluginWorldgenSettlementProfile::PlainsVillagePrototype
    );
    assert!(
        host.client_bundles().is_empty(),
        "neither package ships a client bundle"
    );
    host.stop();
}

#[test]
fn a_deployment_refuses_a_second_declaration_of_one_worldgen_profile() {
    for (kind, declaration) in [
        (
            "ore",
            "\n[worldgen]\nore_profile = \"realistic_deposits\"\n",
        ),
        (
            "settlement",
            "\n[worldgen]\nsettlement_profile = \"plains_village_prototype\"\n",
        ),
    ] {
        let root = tempfile::tempdir().expect("deployment root");
        // The *same* declaration twice: a duplicate has no second slot either, so
        // keeping the later one would be a silent overwrite of a world's profile.
        write_package(root.path(), "first", declaration);
        write_package(root.path(), "second", declaration);
        let limits = PluginLimits::default();
        let configuration = deployment(root.path(), &["first", "second"]);
        let packages = mc_plugin_host::discover(&configuration, &limits)
            .expect("deployment")
            .into_packages();
        let Err(error) = start_deployment(
            packages,
            limits,
            HostQueues::default(),
            Arc::new(NoSessions),
        ) else {
            panic!("two owners of one worldgen profile must not both start");
        };
        let HostStartError::WorldgenConflict {
            kind: refused,
            first,
            second,
        } = error
        else {
            panic!("expected a worldgen conflict, saw {error:?}");
        };
        assert_eq!(refused, kind);
        assert_eq!(first, "first");
        assert_eq!(second, "second");

        // The check refuses it too, through the same aggregation: a deployment
        // that cannot start must not pass a check.
        let error = check_deployment(&configuration, &limits)
            .expect_err("a duplicate worldgen declaration fails the check as well");
        let CheckError::Package { id, message } = error else {
            panic!("the check reports a package refusal, saw {error:?}");
        };
        assert_eq!(id, "second");
        assert!(message.contains(kind), "{message}");
    }
}

#[test]
fn a_check_refuses_two_owners_of_the_world_startup_rules() {
    // The world contract records one startup plan, so the run path refuses a
    // deployment whose two packages both declare rules. A check that reported one
    // of them as fine would bless a deployment that cannot open its world.
    let root = tempfile::tempdir().expect("deployment root");
    for id in ["first", "second"] {
        write_package(root.path(), id, "");
        std::fs::write(
            root.path().join(id).join("config.toml"),
            "mode = \"placement\"\n",
        )
        .expect("config");
    }
    let configuration = deployment(root.path(), &["first", "second"]);
    let error = check_deployment(&configuration, &PluginLimits::default())
        .expect_err("two packages cannot both declare the world's startup rules");
    let CheckError::Package { id, message } = error else {
        panic!("the check reports a package refusal, saw {error:?}");
    };
    assert_eq!(id, "second");
    assert!(message.contains("first"), "{message}");
    assert!(message.contains("second"), "{message}");
}
