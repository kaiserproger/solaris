//! Real-component coverage for the host's pre-commit dispatch path.

mod fixture;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fixture::component_bytes;
use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, NoSessions, PluginLimits, discover,
    start_deployment,
};
use mc_script::ScriptPosition;
use mc_script::precommit::{
    DamageContext, DamageTarget, HookActor, HookContext, HookDecision, HookFailure,
    HookFailurePolicy, HookKind, HookPlayer, HookRegistration,
};

fn write_package(root: &Path, id: &str, config: &str) {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = {id:?}\nname = {id:?}\nversion = \"0.1.0\"\napi = \"0.7.0\"\nhooks = [\"before-build\", \"before-damage\"]\n"
        ),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
    std::fs::write(directory.join("config.toml"), config).expect("config");
}

fn deployment(root: &Path, hooks: Vec<HookRegistration>) -> DeploymentConfig {
    DeploymentConfig {
        root: root.to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: vec!["first".to_owned(), "second".to_owned()],
        grants: BTreeMap::new(),
        require_grants: false,
        precommit_hooks: hooks,
    }
}

fn damage(amount: f32) -> HookContext {
    HookContext::Damage(
        DamageContext::try_new(
            HookActor::Environment,
            DamageTarget::Player(
                HookPlayer::try_new("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", 7).expect("player"),
            ),
            "minecraft:generic",
            "minecraft:overworld",
            ScriptPosition::try_new(0.0, 64.0, 0.0).expect("position"),
            amount,
        )
        .expect("damage"),
    )
}

fn hook(id: &str, order: i32, on_failure: HookFailurePolicy) -> HookRegistration {
    HookRegistration::new(id, HookKind::Damage, order, on_failure)
}

#[tokio::test]
async fn real_components_apply_ordered_cumulative_damage_replacements() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "first",
        "mode = \"precommit\"\ndamage = \"replace\"\ndamage_scale = 1\ndamage_delta = 3\n",
    );
    write_package(
        root.path(),
        "second",
        "mode = \"precommit\"\ndamage = \"replace\"\ndamage_scale = 2\ndamage_delta = 1\n",
    );
    let limits = PluginLimits::default();
    let packages = discover(
        &deployment(
            root.path(),
            vec![
                hook("second", 10, HookFailurePolicy::Deny),
                hook("first", -1, HookFailurePolicy::Deny),
            ],
        ),
        &limits,
    )
    .expect("discovery")
    .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("host starts");

    let pending = host
        .boundary()
        .begin_precommit(damage(20.0))
        .expect("question");
    let mut approval = tokio::time::timeout(Duration::from_secs(10), pending.resolve())
        .await
        .expect("host answers")
        .expect("chain keeps");
    assert_eq!(approval.consume(), Ok(HookDecision::Replace(47.0)));
    host.stop();
}

#[tokio::test]
async fn a_real_component_cancellation_is_terminal() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "first",
        "mode = \"precommit\"\ndamage = \"keep\"\n",
    );
    write_package(
        root.path(),
        "second",
        "mode = \"precommit\"\ndamage = \"cancel\"\n",
    );
    let limits = PluginLimits::default();
    let packages = discover(
        &deployment(
            root.path(),
            vec![
                hook("first", 0, HookFailurePolicy::Deny),
                hook("second", 1, HookFailurePolicy::Deny),
            ],
        ),
        &limits,
    )
    .expect("discovery")
    .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("host starts");

    let pending = host
        .boundary()
        .begin_precommit(damage(20.0))
        .expect("question");
    let mut approval = tokio::time::timeout(Duration::from_secs(10), pending.resolve())
        .await
        .expect("host answers")
        .expect("chain answers");
    assert_eq!(approval.consume(), Err(HookFailure::Cancelled));
    host.stop();
}

async fn deny_fault(fault: &str, limits: PluginLimits) {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "first",
        &format!(
            "mode = \"precommit\"\ndamage = \"keep\"\nhook_fault = {fault:?}\nfault_hook = \"damage\"\n"
        ),
    );
    write_package(
        root.path(),
        "second",
        "mode = \"precommit\"\ndamage = \"keep\"\n",
    );
    let packages = discover(
        &deployment(
            root.path(),
            vec![
                hook("first", 0, HookFailurePolicy::Deny),
                hook("second", 1, HookFailurePolicy::Keep),
            ],
        ),
        &limits,
    )
    .expect("discovery")
    .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("host starts");
    let pending = host
        .boundary()
        .begin_precommit(damage(20.0))
        .expect("question");
    let mut approval = tokio::time::timeout(Duration::from_secs(10), pending.resolve())
        .await
        .expect("host answers")
        .expect("chain answers");
    assert_eq!(approval.consume(), Err(HookFailure::Cancelled));
    let pending = host
        .boundary()
        .begin_precommit(damage(20.0))
        .expect("a later action still asks the mandatory protection");
    let mut approval = pending
        .resolve()
        .await
        .expect("retired handler answers by policy");
    assert_eq!(approval.consume(), Err(HookFailure::Cancelled));
    host.stop();
}

#[tokio::test]
async fn a_real_component_trap_keeps_the_deny_registration_protecting_damage() {
    deny_fault("trap", PluginLimits::default()).await;
}

#[tokio::test]
async fn a_real_component_nonreturning_hook_fails_closed() {
    // Use the deployment's production allowance: the question, not startup,
    // must be what exhausts the budget.
    deny_fault("spin", PluginLimits::default()).await;
}

#[tokio::test]
async fn a_real_component_forbidden_log_import_fails_closed() {
    deny_fault("log", PluginLimits::default()).await;
}

#[tokio::test]
async fn a_retired_optional_handler_preserves_the_previous_replacement() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "first",
        "mode = \"precommit\"\ndamage = \"replace\"\ndamage_scale = 1\ndamage_delta = 3\n",
    );
    write_package(
        root.path(),
        "second",
        "mode = \"precommit\"\nhook_fault = \"trap\"\nfault_hook = \"damage\"\n",
    );
    let limits = PluginLimits::default();
    let packages = discover(
        &deployment(
            root.path(),
            vec![
                hook("first", 0, HookFailurePolicy::Deny),
                hook("second", 1, HookFailurePolicy::Keep),
            ],
        ),
        &limits,
    )
    .expect("discovery")
    .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("host starts");
    for _ in 0..2 {
        let pending = host
            .boundary()
            .begin_precommit(damage(20.0))
            .expect("question");
        let mut approval = pending
            .resolve()
            .await
            .expect("optional failure keeps the action");
        assert_eq!(approval.consume(), Ok(HookDecision::Replace(23.0)));
    }
    host.stop();
}
