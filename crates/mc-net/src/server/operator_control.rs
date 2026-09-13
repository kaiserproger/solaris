//! Typed operator controls. The embedding application owns terminal input.

use std::sync::Arc;

use super::{BoundServer, ShutdownHandle, request_stop};
use crate::{RuntimeControlHandle, chunk_pipeline::ChunkPipelineResources, play};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorWeather {
    Clear,
    Rain,
    Thunder,
}

#[derive(Clone)]
pub struct OperatorControlHandle {
    pub(super) sessions: Arc<play::SessionRegistry>,
    pub(super) simulation: play::SimulationHandle,
    pub(super) shutdown: ShutdownHandle,
    pub(super) runtime_control: Option<RuntimeControlHandle>,
    pub(super) resources: ChunkPipelineResources,
    pub(super) operators: Arc<arc_swap::ArcSwap<std::collections::BTreeSet<String>>>,
    pub(super) whitelist: Arc<arc_swap::ArcSwap<std::collections::BTreeSet<String>>>,
}

impl BoundServer {
    #[must_use]
    pub fn operator_control_handle(&self) -> OperatorControlHandle {
        OperatorControlHandle {
            sessions: Arc::clone(&self.sessions),
            simulation: self.simulation.clone(),
            shutdown: self.config.shutdown.clone(),
            runtime_control: self.runtime_control.clone(),
            resources: self.chunk_pipeline_resources.clone(),
            operators: self.config.command_permissions.operator_identities(),
            whitelist: self.config.command_permissions.whitelist_identities(),
        }
    }
}

impl OperatorControlHandle {
    /// Effective operator identities, sorted.
    #[must_use]
    pub fn operators(&self) -> Vec<String> {
        self.operators.load().iter().cloned().collect()
    }

    /// Effective whitelist identities, sorted.
    #[must_use]
    pub fn whitelist(&self) -> Vec<String> {
        self.whitelist.load().iter().cloned().collect()
    }

    /// Grant or revoke operator authority and return the resulting identities.
    ///
    /// Applies to future logins immediately and to already connected players on
    /// their next command.
    pub fn set_operator(&self, identity: &str, op: bool) -> Vec<String> {
        let identity = identity.trim().to_ascii_lowercase();
        self.operators
            .rcu(|current| Arc::new(identity_set(current, &identity, op)));
        self.operators()
    }

    /// Add or remove a whitelist identity for the next login attempt.
    pub fn set_whitelisted(&self, identity: &str, allowed: bool) -> Vec<String> {
        let identity = identity.trim().to_ascii_lowercase();
        self.whitelist
            .rcu(|current| Arc::new(identity_set(current, &identity, allowed)));
        self.whitelist()
    }

    pub async fn set_world_time(&self, time: u64) -> std::io::Result<()> {
        self.simulation
            .set_world_time_server_owned(time)
            .await
            .map_err(|error| {
                std::io::Error::other(format!("world-time command rejected: {error:?}"))
            })
    }

    pub fn daylight_cycle(&self, value: Option<bool>) -> bool {
        if let Some(value) = value {
            self.sessions.set_daylight_cycle_enabled(value);
        }
        self.sessions.daylight_cycle_enabled()
    }

    pub fn players_sleeping_percentage(&self, value: Option<u32>) -> u32 {
        if let Some(value) = value {
            self.sessions.set_players_sleeping_percentage(value);
        }
        self.sessions.players_sleeping_percentage()
    }

    pub fn set_weather(&self, weather: OperatorWeather) {
        self.sessions.set_weather(match weather {
            OperatorWeather::Clear => play::WeatherKind::Clear,
            OperatorWeather::Rain => play::WeatherKind::Rain,
            OperatorWeather::Thunder => play::WeatherKind::Thunder,
        });
    }

    pub fn request_stop(&self) {
        request_stop(
            &self.shutdown,
            self.runtime_control.as_ref(),
            &self.resources,
            &self.sessions,
        );
    }
}

fn identity_set(
    current: &std::collections::BTreeSet<String>,
    identity: &str,
    present: bool,
) -> std::collections::BTreeSet<String> {
    let mut next = current.clone();
    if present {
        next.insert(identity.to_owned());
    } else {
        next.remove(identity);
    }
    next
}
