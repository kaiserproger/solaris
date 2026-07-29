use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mc_script::{
    AdmittedScriptCommand, ScriptCommand, ScriptDtoError, ScriptVillagerBinding,
    ScriptVillagerBindingFailure, ScriptVillagerGoal, ScriptVillagerGoalFailure,
};
use rsa::rand_core::{OsRng, RngCore};

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use crate::play::SessionRegistry;
use crate::server::ScriptEventSink;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VillagerCommandOutcome {
    pub(crate) accepted: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VillagerAdapterError {
    WrongCommand,
    InvalidResult(ScriptDtoError),
    BindingOwner(mc_entity::RegionOwnerLaneError),
    TokenUnavailable,
    PublicationClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VillagerBindingOwner {
    plugin_id: String,
    expires_at_tick: u64,
}

#[derive(Clone)]
pub(crate) struct PluginVillagerAdapter {
    scripts: ScriptEventSink,
    bindings: Arc<Mutex<BTreeMap<String, VillagerBindingOwner>>>,
}

impl PluginVillagerAdapter {
    pub(crate) fn new(scripts: ScriptEventSink) -> Self {
        Self {
            scripts,
            bindings: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(crate) async fn route_binding_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &SessionRegistry,
    ) -> Result<VillagerCommandOutcome, VillagerAdapterError> {
        let ScriptCommand::RequestVillagerBinding { request } = admitted.request() else {
            return Err(VillagerAdapterError::WrongCommand);
        };
        let request = request.clone();
        let current_tick = sessions.simulation_tick();
        self.purge_expired_bindings(current_tick);

        let binding_id = random_binding_token()?;
        let center = request.center();
        let claim = sessions
            .claim_script_villager_binding(
                mc_entity::Vec3::new(center.x(), center.y(), center.z()),
                request.radius(),
                binding_id,
            )
            .await;
        let (binding, failure) = match claim {
            Ok(Some(claim)) => {
                let binding =
                    ScriptVillagerBinding::try_new(claim.token(), claim.expires_at_tick())
                        .map_err(VillagerAdapterError::InvalidResult)?;
                self.bindings
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(
                        binding.token().to_owned(),
                        VillagerBindingOwner {
                            plugin_id: admitted.plugin_id().to_owned(),
                            expires_at_tick: binding.expires_at_tick(),
                        },
                    );
                (Some(binding), None)
            }
            Ok(None)
            | Err(
                mc_entity::RegionOwnerLaneError::InvalidQuery
                | mc_entity::RegionOwnerLaneError::InvalidMutation,
            ) => (None, Some(ScriptVillagerBindingFailure::NotFound)),
            Err(
                mc_entity::RegionOwnerLaneError::Busy
                | mc_entity::RegionOwnerLaneError::BindingTokenCollision
                | mc_entity::RegionOwnerLaneError::BindingCapacityExceeded,
            ) => (None, Some(ScriptVillagerBindingFailure::Busy)),
            Err(error) => return Err(VillagerAdapterError::BindingOwner(error)),
        };
        let accepted = binding.is_some();
        let event = admitted
            .villager_binding_result(binding, failure)
            .map_err(VillagerAdapterError::InvalidResult)?;
        self.deliver(event).await?;
        Ok(VillagerCommandOutcome { accepted })
    }

    pub(crate) async fn route_goal_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &SessionRegistry,
    ) -> Result<VillagerCommandOutcome, VillagerAdapterError> {
        let ScriptCommand::SetVillagerGoal { request } = admitted.request() else {
            return Err(VillagerAdapterError::WrongCommand);
        };
        let request = request.clone();
        let current_tick = sessions.simulation_tick();
        self.purge_expired_bindings(current_tick);
        let owns_binding = self
            .bindings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(request.binding_token())
            .is_some_and(|binding| binding.plugin_id == admitted.plugin_id());

        let failure = if !owns_binding {
            Some(ScriptVillagerGoalFailure::BindingUnavailable)
        } else {
            let goal = match request.goal() {
                ScriptVillagerGoal::Idle => mc_entity::GoalState::Idle,
                ScriptVillagerGoal::FollowPosition { target, speed_bits } => {
                    mc_entity::GoalState::FollowPosition {
                        target: mc_entity::Vec3::new(target.x(), target.y(), target.z()),
                        speed: f64::from_bits(*speed_bits),
                    }
                }
                _ => {
                    return Err(VillagerAdapterError::InvalidResult(
                        ScriptDtoError::InconsistentResult {
                            field: "villager goal request",
                        },
                    ));
                }
            };
            match sessions
                .apply_script_villager_binding_goal(request.binding_token().to_owned(), goal)
                .await
            {
                Ok(true) => None,
                Ok(false)
                | Err(
                    mc_entity::RegionOwnerLaneError::InvalidQuery
                    | mc_entity::RegionOwnerLaneError::InvalidMutation,
                ) => {
                    self.remove_binding(request.binding_token());
                    Some(ScriptVillagerGoalFailure::BindingUnavailable)
                }
                Err(mc_entity::RegionOwnerLaneError::Busy) => Some(ScriptVillagerGoalFailure::Busy),
                Err(error) => return Err(VillagerAdapterError::BindingOwner(error)),
            }
        };
        let accepted = failure.is_none();
        let event = admitted
            .villager_goal_result(failure)
            .map_err(VillagerAdapterError::InvalidResult)?;
        self.deliver(event).await?;
        Ok(VillagerCommandOutcome { accepted })
    }

    async fn deliver(&self, event: mc_script::ScriptEvent) -> Result<(), VillagerAdapterError> {
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => Ok(()),
            TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                Err(VillagerAdapterError::PublicationClosed)
            }
        }
    }

    fn purge_expired_bindings(&self, current_tick: u64) {
        self.bindings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, binding| current_tick < binding.expires_at_tick);
    }

    fn remove_binding(&self, token: &str) {
        self.bindings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(token);
    }

    #[cfg(test)]
    pub(super) fn binding_owner_for_test(&self, token: &str) -> Option<String> {
        self.bindings
            .lock()
            .ok()?
            .get(token)
            .map(|binding| binding.plugin_id.clone())
    }
}

fn random_binding_token() -> Result<String, VillagerAdapterError> {
    let mut bytes = [0_u8; 16];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| VillagerAdapterError::TokenUnavailable)?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(token)
}
