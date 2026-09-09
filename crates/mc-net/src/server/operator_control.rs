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
        }
    }
}

impl OperatorControlHandle {
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
