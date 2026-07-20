use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mc_script::{AdmittedScriptCommand, ScriptColonyRecord, ScriptCommand, ScriptDtoError};

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use crate::server::ScriptEventSink;

const MAX_COLONIES: usize = 4_096;
const MAX_COLONIES_PER_PLUGIN: usize = 256;

#[derive(Debug, Clone, Copy)]
pub(super) struct ColonyLimits {
    pub(super) total_colonies: usize,
    pub(super) colonies_per_plugin: usize,
}

impl ColonyLimits {
    const fn production() -> Self {
        Self {
            total_colonies: MAX_COLONIES,
            colonies_per_plugin: MAX_COLONIES_PER_PLUGIN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ColonyCommandOutcome {
    pub(crate) accepted: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ColonyAdapterError {
    WrongCommand,
    InvalidResult(ScriptDtoError),
    PublicationClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ColonyKey {
    plugin_id: String,
    colony_id: String,
}

#[derive(Debug)]
struct ColonyRegistry {
    limits: ColonyLimits,
    records: BTreeMap<ColonyKey, ScriptColonyRecord>,
}

impl ColonyRegistry {
    fn new(limits: ColonyLimits) -> Self {
        Self {
            limits,
            records: BTreeMap::new(),
        }
    }

    fn accepts(&self, key: &ColonyKey) -> bool {
        if !self.records.contains_key(key) {
            if self.records.len() >= self.limits.total_colonies {
                return false;
            }
            let plugin_count = self
                .records
                .keys()
                .filter(|existing| existing.plugin_id == key.plugin_id)
                .count();
            if plugin_count >= self.limits.colonies_per_plugin {
                return false;
            }
        }
        true
    }
}

#[derive(Clone)]
pub(crate) struct PluginColonyAdapter {
    scripts: ScriptEventSink,
    registry: Arc<Mutex<ColonyRegistry>>,
}

impl PluginColonyAdapter {
    pub(crate) fn new(scripts: ScriptEventSink) -> Self {
        Self {
            scripts,
            registry: Arc::new(Mutex::new(ColonyRegistry::new(ColonyLimits::production()))),
        }
    }

    #[cfg(test)]
    pub(super) fn with_limits_for_test(scripts: ScriptEventSink, limits: ColonyLimits) -> Self {
        Self {
            scripts,
            registry: Arc::new(Mutex::new(ColonyRegistry::new(limits))),
        }
    }

    pub(crate) async fn route_admitted(
        &self,
        admitted: AdmittedScriptCommand,
    ) -> Result<ColonyCommandOutcome, ColonyAdapterError> {
        let ScriptCommand::UpsertColony { request } = admitted.request() else {
            return Err(ColonyAdapterError::WrongCommand);
        };
        let plugin_id = admitted.plugin_id().to_owned();
        let record = request.record().clone();
        let key = ColonyKey {
            plugin_id,
            colony_id: record.id().to_owned(),
        };
        let (accepted, event) = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let accepted = registry.accepts(&key);
            let event = admitted
                .colony_record_result(accepted)
                .map_err(ColonyAdapterError::InvalidResult)?;
            if accepted {
                registry.records.insert(key, record);
            }
            (accepted, event)
        };
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => Ok(ColonyCommandOutcome { accepted }),
            TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                Err(ColonyAdapterError::PublicationClosed)
            }
        }
    }

    #[cfg(test)]
    pub(super) fn record_for_test(
        &self,
        plugin_id: &str,
        colony_id: &str,
    ) -> Option<ScriptColonyRecord> {
        self.registry
            .lock()
            .ok()?
            .records
            .get(&ColonyKey {
                plugin_id: plugin_id.to_owned(),
                colony_id: colony_id.to_owned(),
            })
            .cloned()
    }
}
