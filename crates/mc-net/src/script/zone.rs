use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use mc_script::{
    AdmittedScriptCommand, ScriptAxisAlignedZone, ScriptCommand, ScriptDtoError, ScriptEvent,
    ScriptPlayerContext, ScriptPlayerId, ScriptPluginTarget,
};

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use crate::server::ScriptEventSink;

const MAX_ZONES: usize = 4_096;
const MAX_ZONES_PER_PLUGIN: usize = 256;
const MAX_TRACKED_PLAYERS: usize = 16_384;
const MAX_ZONE_MEMBERSHIPS: usize = 262_144;
const LAND_CLAIMS_PLUGIN_ID: &str = "land-claims";
const LAND_CLAIM_ZONE_PREFIX: &str = "claim-";

#[derive(Debug, Clone, Copy)]
pub(super) struct ZoneLimits {
    pub(super) total_zones: usize,
    pub(super) zones_per_plugin: usize,
    pub(super) tracked_players: usize,
    pub(super) memberships: usize,
}

impl ZoneLimits {
    pub(super) const fn production() -> Self {
        Self {
            total_zones: MAX_ZONES,
            zones_per_plugin: MAX_ZONES_PER_PLUGIN,
            tracked_players: MAX_TRACKED_PLAYERS,
            memberships: MAX_ZONE_MEMBERSHIPS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZoneCapacity {
    TotalZones,
    ZonesPerPlugin,
    TrackedPlayers,
    Memberships,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ZoneAdapterError {
    WrongCommand,
    InvalidCommand(ScriptDtoError),
    InvalidEvent(ScriptDtoError),
    Full(ZoneCapacity),
    Closed,
    Stale { current_revision: u64 },
    PublicationClosed,
    StateUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZoneCommandOutcome {
    Applied,
    NoOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZoneObservationOutcome {
    Changed { entered: usize, exited: usize },
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ZoneKey {
    plugin_id: String,
    zone_id: String,
}

#[derive(Debug)]
struct RegisteredZone {
    owner: ScriptPluginTarget,
    zone: ScriptAxisAlignedZone,
}

#[derive(Debug)]
struct PlayerMembership {
    revision: u64,
    zones: BTreeSet<ZoneKey>,
}

#[derive(Debug)]
struct ZoneRegistry {
    limits: ZoneLimits,
    closed: bool,
    zones: BTreeMap<ZoneKey, RegisteredZone>,
    players: BTreeMap<u64, PlayerMembership>,
    membership_count: usize,
}

impl ZoneRegistry {
    fn new(limits: ZoneLimits) -> Self {
        Self {
            limits,
            closed: false,
            zones: BTreeMap::new(),
            players: BTreeMap::new(),
            membership_count: 0,
        }
    }

    #[cfg(test)]
    fn route_admitted(
        &mut self,
        admitted: AdmittedScriptCommand,
    ) -> Result<ZoneCommandOutcome, ZoneAdapterError> {
        if self.closed {
            return Err(ZoneAdapterError::Closed);
        }
        match admitted.request() {
            ScriptCommand::UpsertZone { .. } => {
                let (owner, zone) = admitted
                    .into_upsert_zone()
                    .map_err(ZoneAdapterError::InvalidCommand)?;
                self.upsert(owner, zone)
            }
            ScriptCommand::RemoveZone { zone_id } => {
                let key = ZoneKey {
                    plugin_id: admitted.plugin_id().to_owned(),
                    zone_id: zone_id.clone(),
                };
                let ScriptCommand::RemoveZone { .. } = admitted.into_request() else {
                    return Err(ZoneAdapterError::WrongCommand);
                };
                Ok(self.remove(&key))
            }
            _ => Err(ZoneAdapterError::WrongCommand),
        }
    }

    fn upsert(
        &mut self,
        owner: ScriptPluginTarget,
        zone: ScriptAxisAlignedZone,
    ) -> Result<ZoneCommandOutcome, ZoneAdapterError> {
        let key = ZoneKey {
            plugin_id: owner.plugin_id().to_owned(),
            zone_id: zone.id().to_owned(),
        };
        if self
            .zones
            .get(&key)
            .is_some_and(|registered| registered.zone == zone)
        {
            return Ok(ZoneCommandOutcome::NoOp);
        }
        if !self.zones.contains_key(&key) {
            if self.zones.len() >= self.limits.total_zones {
                return Err(ZoneAdapterError::Full(ZoneCapacity::TotalZones));
            }
            let owner_zone_count = self
                .zones
                .keys()
                .filter(|existing| existing.plugin_id == key.plugin_id)
                .count();
            if owner_zone_count >= self.limits.zones_per_plugin {
                return Err(ZoneAdapterError::Full(ZoneCapacity::ZonesPerPlugin));
            }
        }

        self.zones.insert(key, RegisteredZone { owner, zone });
        Ok(ZoneCommandOutcome::Applied)
    }

    fn remove(&mut self, key: &ZoneKey) -> ZoneCommandOutcome {
        if self.zones.remove(key).is_none() {
            return ZoneCommandOutcome::NoOp;
        }
        self.clear_zone_membership(key);
        ZoneCommandOutcome::Applied
    }

    fn clear_zone_membership(&mut self, key: &ZoneKey) {
        for membership in self.players.values_mut() {
            if membership.zones.remove(key) {
                self.membership_count -= 1;
            }
        }
    }

    fn block_mutation_allowed(
        &self,
        actor_uuid: &str,
        operator: bool,
        dimension: &str,
        position: mc_world::BlockPos,
    ) -> bool {
        if operator {
            return true;
        }
        let actor_uuid = actor_uuid
            .bytes()
            .filter(|byte| *byte != b'-')
            .map(char::from)
            .collect::<String>()
            .to_ascii_lowercase();
        self.zones
            .iter()
            .filter(|(key, _)| key.plugin_id == LAND_CLAIMS_PLUGIN_ID)
            .filter(|(_, registered)| zone_contains_block(&registered.zone, dimension, position))
            .filter_map(|(key, _)| claim_owner_uuid(&key.zone_id))
            .all(|owner| owner == actor_uuid)
    }

    fn observe_player(
        &mut self,
        player_id: ScriptPlayerId,
        revision: u64,
        dimension: &str,
        context: &ScriptPlayerContext,
    ) -> Result<(ZoneObservationOutcome, Vec<ScriptEvent>), ZoneAdapterError> {
        if self.closed {
            return Err(ZoneAdapterError::Closed);
        }
        let previous = self.players.get(&player_id.value());
        if let Some(previous) = previous
            && revision <= previous.revision
        {
            return Err(ZoneAdapterError::Stale {
                current_revision: previous.revision,
            });
        }
        if previous.is_none() && self.players.len() >= self.limits.tracked_players {
            return Err(ZoneAdapterError::Full(ZoneCapacity::TrackedPlayers));
        }

        let next_zones = self
            .zones
            .iter()
            .filter(|(_, registered)| contains(registered, dimension, context))
            .map(|(key, _)| key.clone())
            .collect::<BTreeSet<_>>();
        let previous_zones = previous.map(|membership| &membership.zones);
        let previous_count = previous_zones.map_or(0, BTreeSet::len);
        let next_membership_count = self
            .membership_count
            .checked_sub(previous_count)
            .and_then(|count| count.checked_add(next_zones.len()))
            .ok_or(ZoneAdapterError::Full(ZoneCapacity::Memberships))?;
        if next_membership_count > self.limits.memberships {
            return Err(ZoneAdapterError::Full(ZoneCapacity::Memberships));
        }

        let entered = next_zones
            .iter()
            .filter(|key| previous_zones.is_none_or(|zones| !zones.contains(*key)))
            .cloned()
            .collect::<Vec<_>>();
        let exited = previous_zones
            .into_iter()
            .flat_map(|zones| zones.difference(&next_zones))
            .cloned()
            .collect::<Vec<_>>();
        let exited_events = exited.iter().map(|key| {
            let registered = self
                .zones
                .get(key)
                .expect("observed zone must remain registered under registry lock");
            registered
                .owner
                .player_zone_exited(player_id, context.clone(), &registered.zone)
                .map_err(ZoneAdapterError::InvalidEvent)
        });
        let entered_events = entered.iter().map(|key| {
            let registered = self
                .zones
                .get(key)
                .expect("observed zone must remain registered under registry lock");
            registered
                .owner
                .player_zone_entered(player_id, context.clone(), &registered.zone)
                .map_err(ZoneAdapterError::InvalidEvent)
        });
        let events = exited_events
            .chain(entered_events)
            .collect::<Result<Vec<_>, _>>()?;

        self.membership_count = next_membership_count;
        self.players.insert(
            player_id.value(),
            PlayerMembership {
                revision,
                zones: next_zones,
            },
        );
        let outcome = if entered.is_empty() && exited.is_empty() {
            ZoneObservationOutcome::NoOp
        } else {
            ZoneObservationOutcome::Changed {
                entered: entered.len(),
                exited: exited.len(),
            }
        };
        Ok((outcome, events))
    }

    fn forget_player(
        &mut self,
        player_id: ScriptPlayerId,
    ) -> Result<ZoneCommandOutcome, ZoneAdapterError> {
        if self.closed {
            return Err(ZoneAdapterError::Closed);
        }
        let Some(membership) = self.players.remove(&player_id.value()) else {
            return Ok(ZoneCommandOutcome::NoOp);
        };
        self.membership_count -= membership.zones.len();
        Ok(ZoneCommandOutcome::Applied)
    }

    fn close(&mut self) {
        self.closed = true;
        self.zones.clear();
        self.players.clear();
        self.membership_count = 0;
    }
}

fn contains(registered: &RegisteredZone, dimension: &str, context: &ScriptPlayerContext) -> bool {
    if registered.zone.dimension() != dimension {
        return false;
    }
    let minimum = registered.zone.minimum();
    let maximum = registered.zone.maximum();
    context.x() >= minimum.x()
        && context.x() <= maximum.x()
        && context.y() >= minimum.y()
        && context.y() <= maximum.y()
        && context.z() >= minimum.z()
        && context.z() <= maximum.z()
}

#[derive(Clone)]
pub(crate) struct PluginZoneAdapter {
    scripts: ScriptEventSink,
    registry: Arc<Mutex<ZoneRegistry>>,
}

impl PluginZoneAdapter {
    pub(crate) fn new(scripts: ScriptEventSink) -> Self {
        Self {
            scripts,
            registry: Arc::new(Mutex::new(ZoneRegistry::new(ZoneLimits::production()))),
        }
    }

    #[cfg(test)]
    pub(super) fn with_limits_for_test(scripts: ScriptEventSink, limits: ZoneLimits) -> Self {
        Self {
            scripts,
            registry: Arc::new(Mutex::new(ZoneRegistry::new(limits))),
        }
    }

    #[cfg(test)]
    pub(crate) fn route_admitted(
        &self,
        admitted: AdmittedScriptCommand,
    ) -> Result<ZoneCommandOutcome, ZoneAdapterError> {
        self.registry
            .lock()
            .map_err(|_| ZoneAdapterError::StateUnavailable)?
            .route_admitted(admitted)
    }

    pub(crate) async fn route_admitted_with_result(
        &self,
        admitted: AdmittedScriptCommand,
    ) -> Result<ZoneCommandOutcome, ZoneAdapterError> {
        let (target, zone_id, outcome) = match admitted.request() {
            ScriptCommand::UpsertZone { .. } => {
                let (target, zone) = admitted
                    .into_upsert_zone()
                    .map_err(ZoneAdapterError::InvalidCommand)?;
                let zone_id = zone.id().to_owned();
                let outcome = self
                    .registry
                    .lock()
                    .map_err(|_| ZoneAdapterError::StateUnavailable)
                    .and_then(|mut registry| registry.upsert(target.clone(), zone));
                (target, zone_id, outcome)
            }
            ScriptCommand::RemoveZone { .. } => {
                let (target, zone_id) = admitted
                    .into_remove_zone()
                    .map_err(ZoneAdapterError::InvalidCommand)?;
                let key = ZoneKey {
                    plugin_id: target.plugin_id().to_owned(),
                    zone_id: zone_id.clone(),
                };
                let outcome = self
                    .registry
                    .lock()
                    .map_err(|_| ZoneAdapterError::StateUnavailable)
                    .and_then(|mut registry| {
                        if registry.closed {
                            Err(ZoneAdapterError::Closed)
                        } else {
                            Ok(registry.remove(&key))
                        }
                    });
                (target, zone_id, outcome)
            }
            _ => return Err(ZoneAdapterError::WrongCommand),
        };
        let event = target
            .zone_command_result(&zone_id, outcome.is_ok())
            .map_err(ZoneAdapterError::InvalidEvent)?;
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => outcome,
            TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                Err(ZoneAdapterError::PublicationClosed)
            }
        }
    }

    pub(crate) async fn observe_player(
        &self,
        player_id: ScriptPlayerId,
        revision: u64,
        dimension: &str,
        context: ScriptPlayerContext,
    ) -> Result<ZoneObservationOutcome, ZoneAdapterError> {
        let (outcome, events) = self
            .registry
            .lock()
            .map_err(|_| ZoneAdapterError::StateUnavailable)?
            .observe_player(player_id, revision, dimension, &context)?;
        for event in events {
            match deliver_required_targeted_event(&self.scripts, event).await {
                TargetedEventDelivery::Delivered => {}
                TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                    return Err(ZoneAdapterError::PublicationClosed);
                }
            }
        }
        Ok(outcome)
    }

    pub(crate) fn forget_player(
        &self,
        player_id: ScriptPlayerId,
    ) -> Result<ZoneCommandOutcome, ZoneAdapterError> {
        self.registry
            .lock()
            .map_err(|_| ZoneAdapterError::StateUnavailable)?
            .forget_player(player_id)
    }

    pub(crate) fn block_mutation_allowed(
        &self,
        actor_uuid: &str,
        operator: bool,
        dimension: &str,
        position: mc_world::BlockPos,
    ) -> Result<bool, ZoneAdapterError> {
        Ok(self
            .registry
            .lock()
            .map_err(|_| ZoneAdapterError::StateUnavailable)?
            .block_mutation_allowed(actor_uuid, operator, dimension, position))
    }

    pub(crate) fn close(&self) -> Result<(), ZoneAdapterError> {
        self.registry
            .lock()
            .map_err(|_| ZoneAdapterError::StateUnavailable)?
            .close();
        Ok(())
    }
}

fn zone_contains_block(
    zone: &ScriptAxisAlignedZone,
    dimension: &str,
    position: mc_world::BlockPos,
) -> bool {
    if zone.dimension() != dimension {
        return false;
    }
    let minimum = zone.minimum();
    let maximum = zone.maximum();
    f64::from(position.x) >= minimum.x()
        && f64::from(position.x) <= maximum.x()
        && f64::from(position.y) >= minimum.y()
        && f64::from(position.y) <= maximum.y()
        && f64::from(position.z) >= minimum.z()
        && f64::from(position.z) <= maximum.z()
}

fn claim_owner_uuid(zone_id: &str) -> Option<&str> {
    let mut parts = zone_id.strip_prefix(LAND_CLAIM_ZONE_PREFIX)?.split('-');
    let owner = parts.next()?;
    let chunk_x = parts.next()?;
    let chunk_z = parts.next()?;
    let valid = parts.next().is_none()
        && owner.len() == 32
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        && valid_claim_coordinate_id(chunk_x)
        && valid_claim_coordinate_id(chunk_z);
    valid.then_some(owner)
}

fn valid_claim_coordinate_id(value: &str) -> bool {
    matches!(value.as_bytes().first(), Some(b'p' | b'n'))
        && value.len() > 1
        && value.as_bytes()[1..].iter().all(u8::is_ascii_digit)
}
