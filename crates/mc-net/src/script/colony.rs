use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mc_script::{
    AdmittedScriptCommand, ScriptColonyRecord, ScriptCommand, ScriptDtoError,
    ScriptVillagerBinding, ScriptVillagerOrder,
};
use rsa::rand_core::{OsRng, RngCore};

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use crate::play::SessionRegistry;
use crate::server::ScriptEventSink;

const MAX_COLONIES: usize = 4_096;
const MAX_COLONIES_PER_PLUGIN: usize = 256;
const VILLAGER_HOME_SPEED: f64 = 0.3;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingGoalApplication {
    Applied,
    Rejected,
    Retryable,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ColonyAdapterError {
    WrongCommand,
    InvalidResult(ScriptDtoError),
    BindingOwner(mc_entity::RegionOwnerLaneError),
    TokenUnavailable,
    PublicationClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ColonyKey {
    plugin_id: String,
    colony_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VillagerBindingOwner {
    colony: ColonyKey,
    expires_at_tick: u64,
}

#[derive(Debug)]
struct ColonyRegistry {
    limits: ColonyLimits,
    records: BTreeMap<ColonyKey, ScriptColonyRecord>,
    villager_bindings: BTreeMap<String, VillagerBindingOwner>,
}

impl ColonyRegistry {
    fn new(limits: ColonyLimits) -> Self {
        Self {
            limits,
            records: BTreeMap::new(),
            villager_bindings: BTreeMap::new(),
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

    fn purge_expired_villager_bindings(&mut self, current_tick: u64) {
        self.villager_bindings
            .retain(|_, binding| current_tick < binding.expires_at_tick);
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

    pub(crate) async fn route_binding_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &SessionRegistry,
    ) -> Result<ColonyCommandOutcome, ColonyAdapterError> {
        let ScriptCommand::RequestVillagerBinding { request } = admitted.request() else {
            return Err(ColonyAdapterError::WrongCommand);
        };
        let request = request.clone();
        let key = ColonyKey {
            plugin_id: admitted.plugin_id().to_owned(),
            colony_id: request.colony_id().to_owned(),
        };
        let current_tick = sessions.simulation_tick();
        let eligible = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            registry.purge_expired_villager_bindings(current_tick);
            registry
                .records
                .get(&key)
                .is_some_and(|record| record.dimension() == "minecraft:overworld")
        };

        let binding = if eligible {
            let token = random_binding_token()?;
            let center = request.center();
            classify_binding_claim(
                sessions
                    .claim_script_villager_binding(
                        mc_entity::Vec3::new(center.x(), center.y(), center.z()),
                        request.radius(),
                        token,
                    )
                    .await,
            )?
            .map(|claim| ScriptVillagerBinding::try_new(claim.token(), claim.expires_at_tick()))
            .transpose()
            .map_err(ColonyAdapterError::InvalidResult)?
        } else {
            None
        };
        let accepted = binding.is_some();
        if let Some(binding) = &binding {
            self.registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .villager_bindings
                .insert(
                    binding.token().to_owned(),
                    VillagerBindingOwner {
                        colony: key,
                        expires_at_tick: binding.expires_at_tick(),
                    },
                );
        }
        let event = admitted
            .colony_villager_binding_result(binding)
            .map_err(ColonyAdapterError::InvalidResult)?;
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => Ok(ColonyCommandOutcome { accepted }),
            TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                Err(ColonyAdapterError::PublicationClosed)
            }
        }
    }

    pub(crate) async fn route_order_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &SessionRegistry,
    ) -> Result<ColonyCommandOutcome, ColonyAdapterError> {
        let ScriptCommand::SetVillagerOrder { request } = admitted.request() else {
            return Err(ColonyAdapterError::WrongCommand);
        };
        let request = request.clone();
        let key = ColonyKey {
            plugin_id: admitted.plugin_id().to_owned(),
            colony_id: request.colony_id().to_owned(),
        };
        let current_tick = sessions.simulation_tick();
        let (goal, owns_binding) = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            registry.purge_expired_villager_bindings(current_tick);
            let owns_binding = registry
                .villager_bindings
                .get(request.binding_token())
                .is_some_and(|binding| binding.colony == key);
            let record = registry.records.get(&key);
            if !owns_binding {
                (None, false)
            } else {
                let goal = match (request.order(), record) {
                    (ScriptVillagerOrder::Hold, Some(record))
                        if record.dimension() == "minecraft:overworld" =>
                    {
                        Some(mc_entity::GoalState::Idle)
                    }
                    (ScriptVillagerOrder::Home, Some(record))
                        if record.dimension() == "minecraft:overworld" =>
                    {
                        let home = record.home();
                        Some(mc_entity::GoalState::FollowPosition {
                            target: mc_entity::Vec3::new(home.x(), home.y(), home.z()),
                            speed: VILLAGER_HOME_SPEED,
                        })
                    }
                    _ => None,
                };
                (goal, true)
            }
        };

        let application = if let Some(goal) = goal {
            classify_binding_goal_application(
                sessions
                    .apply_script_villager_binding_goal(request.binding_token().to_owned(), goal)
                    .await,
            )?
        } else {
            BindingGoalApplication::Rejected
        };
        let accepted = application == BindingGoalApplication::Applied;
        if owns_binding && application == BindingGoalApplication::Rejected {
            self.registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .villager_bindings
                .remove(request.binding_token());
        }
        let event = admitted
            .colony_villager_order_result(accepted)
            .map_err(ColonyAdapterError::InvalidResult)?;
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

pub(super) fn classify_binding_claim(
    result: Result<Option<mc_entity::VillagerBindingClaim>, mc_entity::RegionOwnerLaneError>,
) -> Result<Option<mc_entity::VillagerBindingClaim>, ColonyAdapterError> {
    match result {
        Ok(claim) => Ok(claim),
        Err(
            mc_entity::RegionOwnerLaneError::InvalidQuery
            | mc_entity::RegionOwnerLaneError::BindingTokenCollision
            | mc_entity::RegionOwnerLaneError::BindingCapacityExceeded
            | mc_entity::RegionOwnerLaneError::Busy,
        ) => Ok(None),
        Err(error) => Err(ColonyAdapterError::BindingOwner(error)),
    }
}

pub(super) fn classify_binding_goal_application(
    result: Result<bool, mc_entity::RegionOwnerLaneError>,
) -> Result<BindingGoalApplication, ColonyAdapterError> {
    match result {
        Ok(true) => Ok(BindingGoalApplication::Applied),
        Ok(false) => Ok(BindingGoalApplication::Rejected),
        Err(mc_entity::RegionOwnerLaneError::Busy) => Ok(BindingGoalApplication::Retryable),
        Err(
            mc_entity::RegionOwnerLaneError::InvalidQuery
            | mc_entity::RegionOwnerLaneError::InvalidMutation,
        ) => Ok(BindingGoalApplication::Rejected),
        Err(error) => Err(ColonyAdapterError::BindingOwner(error)),
    }
}

fn random_binding_token() -> Result<String, ColonyAdapterError> {
    let mut bytes = [0_u8; 16];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| ColonyAdapterError::TokenUnavailable)?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(token)
}
