use super::*;

/// Build the component deployment for a configured plugin directory.
///
/// The operator's `[[plugins.hooks]]` registrations travel with the component
/// deployment after `PluginSection::validate_hooks` has established that it has
/// a directory to attach them to.
pub(crate) fn component_deployment(
    config: &ServerConfig,
) -> Result<Option<mc_plugin_host::DeploymentConfig>> {
    if !config.plugins.strict && !config.plugins.expected.is_empty() {
        bail!("plugins.expected requires plugins.strict = true");
    }
    let Some(directory) = config.plugins.directory.clone() else {
        return Ok(None);
    };
    let mode = if config.plugins.strict {
        mc_plugin_host::DiscoveryMode::Strict
    } else {
        mc_plugin_host::DiscoveryMode::Permissive
    };
    let grants = config
        .plugins
        .grants
        .iter()
        .map(|(id, grant)| (id.clone(), grant.capabilities.clone()))
        .collect();
    // The registrations discovery validates against the deployed packages and
    // attaches to the ones that must answer them; the host's roster is built from
    // those attachments, not from the manifest declarations behind them.
    let precommit_hooks: Vec<mc_script::precommit::HookRegistration> = config
        .plugins
        .hooks
        .iter()
        .map(PluginHookSection::registration)
        .collect();
    Ok(Some(mc_plugin_host::DeploymentConfig {
        root: directory,
        mode,
        expected: config.plugins.expected.clone(),
        grants,
        // A strict production deployment must have granted what it runs; local
        // iteration may run without operator grants.
        require_grants: config.plugins.strict,
        precommit_hooks,
    }))
}

/// A started component deployment and what the world needs from it.
///
/// The host is started here rather than next to the network bind because its
/// `configure` phase is what produces the startup rules the world is opened with,
/// and because it owns the boundary the network binds to. Nothing is bound yet at
/// this point: the host's opening commands wait in the boundary queue.
pub(crate) struct PreparedComponent {
    /// The running host, or `None` once the server has taken it.
    pub(crate) host: Option<mc_plugin_host::PluginHost>,
    /// The ids this deployment discovered, in the host's own order.
    pub(crate) plugin_ids: Vec<String>,
    /// The validated startup rules, or `None` when no package declared any.
    pub(crate) rules: Option<mc_script::GameplayRules>,
    /// The ore profile the deployment's packages declared, or `None` when none
    /// did. Read back from the started host, where `configure` ran.
    pub(crate) ore_profile: Option<mc_script::PluginWorldgenOreProfile>,
    /// The settlement plan the deployment's packages declared, read back the
    /// same way.
    pub(crate) settlement_plan: Option<mc_script::PluginSettlementPlan>,
    /// The client bundles this deployment ships, in the host's own order.
    pub(crate) client_bundles: Vec<mc_script::ClientBundle>,
    /// The server's live sessions. The host holds the lookup before the server
    /// exists; the server fills it when it binds.
    pub(crate) sessions: mc_net::PlayerSessionsHandle,
}

impl PreparedComponent {
    /// Take the running host: whoever takes it owns stopping it.
    ///
    /// The server takes it when it binds; a startup that fails before the bind
    /// drops this value instead, which stops the same host.
    pub(crate) fn take_host(&mut self) -> mc_plugin_host::PluginHost {
        self.host
            .take()
            .expect("the prepared component host is taken exactly once")
    }
}

impl Drop for PreparedComponent {
    /// Stop a host startup never handed to a server.
    ///
    /// Everything between the host starting and the network binding - reading
    /// the Loader manifest, loading the startup data, validating the world
    /// contract, opening the world - can fail, and each of those failures drops
    /// this value on the way out. The host is a live thread that owns boundary
    /// queues and every guest's durable storage, so it is stopped here, exactly
    /// as the bind-failure path stops it, and what it owned is reported the same
    /// way; a host the server did take is `None` by then and this is a no-op.
    fn drop(&mut self) {
        if let Some(host) = self.host.take() {
            report_component_stop(host.stop());
        }
    }
}

/// The component host's view of this server's live sessions.
///
/// The host addresses a player by stable identity; the server owns which session
/// that identity holds right now, so this is a pass-through with no table of its
/// own. A player who is offline resolves to nothing and their command is refused,
/// never delivered to whoever holds that runtime id next.
pub(crate) struct ServerSessions(mc_net::PlayerSessionsHandle);

impl mc_plugin_host::PlayerSessions for ServerSessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        self.0.session_of(player)
    }
}

/// Discover, start and read back one component deployment.
///
/// Failure modes are all startup failures: a directory that cannot be read in
/// strict mode, a package that asks for rights the operator did not grant, a
/// package whose `configure` never returns, and a rule plan the host refuses. A
/// deployment that declared rules it cannot get would run a world it did not
/// configure, so a refusal stops the server instead of being logged.
pub(crate) async fn prepare_component_deployment(
    config: &ServerConfig,
) -> Result<Option<PreparedComponent>> {
    let Some(deployment) = component_deployment(config)? else {
        return Ok(None);
    };
    // The plan's numbers come from measurement (P7); until then a component
    // deployment runs on the host's documented defaults rather than on an
    // operator surface nobody has calibrated.
    let limits = mc_plugin_host::PluginLimits::default();
    let discovered = mc_plugin_host::discover(&deployment, &limits).with_context(|| {
        format!(
            "reading the configured component plugins from {}",
            deployment.root.display()
        )
    })?;
    for skipped in discovered.skipped() {
        // Permissive mode skips an ordinary broken package; strict mode fails in
        // `discover` instead. An operator has to see which one was left out.
        tracing::warn!(
            path = %skipped.path.display(),
            message = skipped.message,
            "component plugin skipped"
        );
    }
    let packages = discovered.into_packages();
    let plugin_ids = packages
        .iter()
        .map(|package| package.manifest().plugin_id().to_owned())
        .collect::<Vec<_>>();
    let sessions = mc_net::PlayerSessionsHandle::new();
    let host = mc_plugin_host::start_deployment(
        packages,
        limits,
        mc_plugin_host::HostQueues::default(),
        std::sync::Arc::new(ServerSessions(sessions.clone())),
    )
    .map_err(|error| anyhow::anyhow!("starting the component plugin host: {error}"))?;

    // Read the contribution before anything else can fail, and stop the host on
    // every refusal: a server that opens a world and then refuses the rules it
    // was configured with has already changed durable state.
    let refusal = host
        .contribution()
        .refusal()
        .map(|(id, refusal)| (id.to_owned(), refusal.to_string()));
    let declared = host
        .contribution()
        .rules()
        .map(|(id, rules)| (id.to_owned(), rules.clone()))
        .collect::<Vec<_>>();
    if let Some((id, refusal)) = refusal {
        stop_component_host(host).await?;
        bail!("component plugin {id} declared a rule plan the host refused: {refusal}");
    }
    let rules = match declared.len() {
        0 => None,
        1 => Some(declared.into_iter().next().expect("one entry").1),
        _ => {
            let owners = declared
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            stop_component_host(host).await?;
            bail!(
                "component plugins {owners} each declare startup rules; the world contract records one plan, so exactly one package may declare them"
            );
        }
    };
    // Read the world-startup records from the host that ran `configure` before it
    // can be handed to a server. The host refuses a second declaration of an ore
    // or settlement profile while it starts, so a deployment that reaches here
    // has at most one owner per profile.
    let ore_profile = host.worldgen_ore_profile();
    let settlement_plan = host.worldgen_settlement_plan().cloned();
    let client_bundles = host.client_bundles().to_vec();
    Ok(Some(PreparedComponent {
        host: Some(host),
        plugin_ids,
        rules,
        ore_profile,
        settlement_plan,
        client_bundles,
        sessions,
    }))
}

/// Stop a component host and report what it owned, off the runtime's thread.
pub(crate) async fn stop_component_host(host: mc_plugin_host::PluginHost) -> Result<()> {
    let counters = tokio::task::spawn_blocking(move || host.stop())
        .await
        .context("stopping the component plugin host")?;
    report_component_stop(counters);
    Ok(())
}

/// Report what a stopped host owned.
///
/// Every stop reports the same counters, whether a finished run asked for it or a
/// startup that failed dropped the host it had prepared.
pub(crate) fn report_component_stop(counters: Vec<(String, mc_plugin_host::InstanceDiagnostics)>) {
    for (plugin_id, diagnostics) in counters {
        tracing::info!(
            plugin_id,
            calls = diagnostics.calls,
            events_delivered = diagnostics.events_delivered,
            commands_submitted = diagnostics.commands_submitted,
            commands_refused = diagnostics.commands_refused,
            "component plugin stopped"
        );
    }
}
