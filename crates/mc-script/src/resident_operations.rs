//! Durable resident identity, lifecycle, and POI binding DTOs.
//!
//! These types are the closed wire contract for `solaris.claim_resident`,
//! `solaris.spawn_resident`, `solaris.query_residents`, `solaris.release_resident`
//! and `solaris.set_resident_pois`. They deliberately carry no authoritative
//! world state: owner comes from the admitted plugin, actor from the
//! authenticated session, and the entity from core's own durable binding.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    MAX_SCRIPT_WORLD_TIME, ScriptDtoError, ScriptPosition, validate_bounded_nonempty,
    validate_bounded_value, validate_script_id,
};

/// Maximum number of opaque handles one `query_residents` call may address.
pub const MAX_RESIDENT_QUERY_HANDLES: usize = 64;
/// Maximum number of resident records returned by one query page.
pub const MAX_RESIDENT_PAGE: usize = 64;
/// Maximum byte length of an opaque resident handle.
pub const MAX_RESIDENT_HANDLE_BYTES: usize = 64;
/// Maximum byte length of a canonical entity UUID string.
pub const MAX_RESIDENT_ENTITY_UUID_BYTES: usize = 36;
/// Maximum byte length of a deterministic inhabitant generation id.
pub const MAX_RESIDENT_GENERATION_ID_BYTES: usize = 64;
/// Maximum byte length of an owner-scoped POI handle.
pub const MAX_RESIDENT_POI_HANDLE_BYTES: usize = 128;
/// Maximum byte length of an opaque spawn site token.
pub const MAX_RESIDENT_SPAWN_TOKEN_BYTES: usize = 64;
/// Maximum carried item summaries exposed by one resident snapshot.
pub const MAX_RESIDENT_CARRIED_ITEMS: usize = 8;
/// Maximum distance between the claiming actor and the adopted entity.
pub const MAX_RESIDENT_CLAIM_DISTANCE: f64 = 64.0;
/// Maximum resident records (living plus tombstones) one plugin may hold.
pub const MAX_RESIDENT_RECORDS_PER_PLUGIN: usize = 256;
/// Maximum living residents one plugin may hold; dead tombstones do not count.
pub const MAX_RESIDENT_LIVE_PER_PLUGIN: usize = 64;

const GENERATION_ID_BYTES: usize = 32;

/// Closed lifecycle state of one durable resident handle.
///
/// `alive_unloaded` is not `dead`: an unloaded resident keeps its identity and
/// living capacity, while a dead resident leaves a tombstone but frees living
/// capacity. Neither state ever justifies rebinding the handle to another
/// entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptResidentLifecycle {
    AliveLoaded,
    AliveUnloaded,
    Dead,
    Released,
}

impl ScriptResidentLifecycle {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AliveLoaded => "alive_loaded",
            Self::AliveUnloaded => "alive_unloaded",
            Self::Dead => "dead",
            Self::Released => "released",
        }
    }

    /// Whether this state still occupies living population capacity.
    pub const fn occupies_living_capacity(self) -> bool {
        matches!(self, Self::AliveLoaded | Self::AliveUnloaded)
    }
}

/// Allowed resident kinds for a bounded `spawn_resident` profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptResidentKind {
    Villager,
}

impl ScriptResidentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Villager => "villager",
        }
    }
}

/// Bounded core-owned description of the resident to materialise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptResidentProfile {
    kind: ScriptResidentKind,
}

impl ScriptResidentProfile {
    #[must_use]
    pub const fn new(kind: ScriptResidentKind) -> Self {
        Self { kind }
    }

    pub const fn kind(&self) -> ScriptResidentKind {
        self.kind
    }
}

/// Owner-scoped physical POI handles bound to one resident.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptResidentPois {
    pub home: Option<String>,
    pub work: Option<String>,
    pub meeting: Option<String>,
}

impl ScriptResidentPois {
    #[must_use]
    pub fn new(home: Option<String>, work: Option<String>, meeting: Option<String>) -> Self {
        Self {
            home,
            work,
            meeting,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        for poi in [&self.home, &self.work, &self.meeting]
            .into_iter()
            .flatten()
        {
            validate_bounded_nonempty("resident poi", poi, MAX_RESIDENT_POI_HANDLE_BYTES)?;
        }
        Ok(())
    }
}

/// One carried item summary read from the live entity snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptResidentItemSummary {
    pub item_id: String,
    pub count: u32,
}

impl ScriptResidentItemSummary {
    #[must_use]
    pub fn new(item_id: String, count: u32) -> Self {
        Self { item_id, count }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::check_contract_resource_id(&self.item_id)?;
        if self.count == 0 || self.count > i32::MAX as u32 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Live entity state for a resident whose chunk is currently loaded.
///
/// `Eq` is implemented by hand because the pose and health are floats: every
/// value is validated finite before it becomes durable, so the marker trait
/// cannot silently accept a NaN identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptResidentLoadedState {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub health: f32,
    pub carried: Vec<ScriptResidentItemSummary>,
}

impl Eq for ScriptResidentLoadedState {}

impl ScriptResidentLoadedState {
    pub fn new(
        position: ScriptPosition,
        health: f32,
        carried: Vec<ScriptResidentItemSummary>,
    ) -> Self {
        Self {
            x: position.x(),
            y: position.y(),
            z: position.z(),
            health,
            carried,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if ScriptPosition::try_new(self.x, self.y, self.z).is_none()
            || !self.health.is_finite()
            || self.health < 0.0
            || self.carried.len() > MAX_RESIDENT_CARRIED_ITEMS
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        for item in &self.carried {
            item.validate()?;
        }
        Ok(())
    }
}

/// Bounded core-owned snapshot of one durable resident handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptResidentSnapshot {
    pub handle: String,
    pub entity_uuid: String,
    pub lifecycle: ScriptResidentLifecycle,
    pub revision: u64,
    pub generation_id: Option<String>,
    pub pois: ScriptResidentPois,
    pub loaded: Option<ScriptResidentLoadedState>,
}

impl ScriptResidentSnapshot {
    #[must_use]
    pub fn new(
        handle: String,
        entity_uuid: String,
        lifecycle: ScriptResidentLifecycle,
        revision: u64,
        generation_id: Option<String>,
        pois: ScriptResidentPois,
        loaded: Option<ScriptResidentLoadedState>,
    ) -> Self {
        Self {
            handle,
            entity_uuid,
            lifecycle,
            revision,
            generation_id,
            pois,
            loaded,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_resident_handle(&self.handle)?;
        validate_entity_uuid(&self.entity_uuid)?;
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || (self.lifecycle == ScriptResidentLifecycle::AliveLoaded) != self.loaded.is_some()
        {
            return Err(ScriptDtoError::InconsistentResult {
                field: "resident snapshot",
            });
        }
        if let Some(generation_id) = &self.generation_id {
            validate_bounded_nonempty(
                "resident generation id",
                generation_id,
                MAX_RESIDENT_GENERATION_ID_BYTES,
            )?;
        }
        self.pois.validate()?;
        if let Some(loaded) = &self.loaded {
            loaded.validate()?;
        }
        Ok(())
    }
}

/// Closed typed payload for one committed resident mutation or query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptResidentResult {
    Snapshot {
        resident: ScriptResidentSnapshot,
    },
    Page {
        residents: Vec<ScriptResidentSnapshot>,
        cursor: Option<String>,
    },
}

impl ScriptResidentResult {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Snapshot { resident } => resident.validate(),
            Self::Page { residents, cursor } => {
                if residents.len() > MAX_RESIDENT_PAGE {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "resident page",
                        max: MAX_RESIDENT_PAGE,
                    });
                }
                let mut previous = None;
                for resident in residents {
                    resident.validate()?;
                    if previous.is_some_and(|handle: &str| handle >= resident.handle.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "resident page order",
                        });
                    }
                    previous = Some(resident.handle.as_str());
                }
                if let Some(cursor) = cursor {
                    validate_resident_handle(cursor)?;
                }
                Ok(())
            }
        }
    }
}

/// One bounded resident call inside the durable operation envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptResidentOperation {
    Claim {
        operation_id: String,
        actor_id: u64,
        entity_uuid: String,
        expected_entity_revision: u64,
    },
    Spawn {
        operation_id: String,
        spawn_site_token: String,
        profile: ScriptResidentProfile,
    },
    Query {
        handles: Vec<String>,
        cursor: Option<String>,
    },
    Release {
        operation_id: String,
        handle: String,
        expected_revision: u64,
    },
    SetPois {
        operation_id: String,
        handle: String,
        home_poi: Option<String>,
        work_poi: Option<String>,
        meeting_poi: Option<String>,
        expected_revision: u64,
    },
}

impl ScriptResidentOperation {
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Claim { operation_id, .. }
            | Self::Spawn { operation_id, .. }
            | Self::Release { operation_id, .. }
            | Self::SetPois { operation_id, .. } => Some(operation_id),
            Self::Query { .. } => None,
        }
    }

    /// Canonical ordering so one operation id always fingerprints identically.
    pub fn canonicalize(&mut self) {
        if let Self::Query { handles, .. } = self {
            handles.sort_unstable();
            handles.dedup();
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(operation_id) = self.operation_id() {
            validate_script_id(operation_id)?;
        }
        match self {
            Self::Claim {
                actor_id,
                entity_uuid,
                expected_entity_revision,
                ..
            } => {
                if *actor_id == 0 || *actor_id > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                validate_entity_uuid(entity_uuid)?;
                if *expected_entity_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Spawn {
                spawn_site_token, ..
            } => validate_bounded_nonempty(
                "spawn site token",
                spawn_site_token,
                MAX_RESIDENT_SPAWN_TOKEN_BYTES,
            ),
            Self::Query { handles, cursor } => {
                if handles.len() > MAX_RESIDENT_QUERY_HANDLES {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "resident query handles",
                        max: MAX_RESIDENT_QUERY_HANDLES,
                    });
                }
                let mut previous = None;
                for handle in handles {
                    validate_resident_handle(handle)?;
                    if previous.is_some_and(|value: &str| value >= handle.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "resident query order",
                        });
                    }
                    previous = Some(handle.as_str());
                }
                if let Some(cursor) = cursor {
                    validate_resident_handle(cursor)?;
                }
                Ok(())
            }
            Self::Release {
                handle,
                expected_revision,
                ..
            } => {
                validate_resident_handle(handle)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::SetPois {
                handle,
                home_poi,
                work_poi,
                meeting_poi,
                expected_revision,
                ..
            } => {
                validate_resident_handle(handle)?;
                ScriptResidentPois {
                    home: home_poi.clone(),
                    work: work_poi.clone(),
                    meeting: meeting_poi.clone(),
                }
                .validate()?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
        }
    }
}

pub fn validate_resident_handle(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("resident handle", value, MAX_RESIDENT_HANDLE_BYTES)
}

pub fn validate_entity_uuid(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty(
        "resident entity uuid",
        value,
        MAX_RESIDENT_ENTITY_UUID_BYTES,
    )?;
    if value.len() != 36
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
    {
        return Err(ScriptDtoError::InvalidId {
            field: "resident entity uuid",
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

/// Derive the deterministic inhabitant identity from world identity, site id,
/// and inhabitant slot.
///
/// The result is a stable, opaque generation id: regenerating the same world
/// with the same profile always yields the same ids, and core registers the
/// generation id to entity UUID binding before any change notification. This is
/// a narrow derivation, not a scheduler.
///
/// The world identity is the server's canonical world directory, so it is a
/// filesystem path and not bounded by the 128-byte plugin-identity limit. The
/// fixed-width hash below keeps the derived generation id bounded (64 hex
/// bytes) for any directory depth while staying reproducible across restarts.
pub fn resident_generation_id(
    world_identity: &str,
    site_id: &str,
    inhabitant_slot: u32,
) -> Result<String, ScriptDtoError> {
    if world_identity.is_empty() {
        return Err(ScriptDtoError::EmptyValue {
            field: "world identity",
        });
    }
    validate_bounded_nonempty("settlement site id", site_id, 128)?;
    let mut hasher = Sha256::new();
    hasher.update(b"solaris.resident.generation.v1");
    hasher.update((world_identity.len() as u32).to_le_bytes());
    hasher.update(world_identity.as_bytes());
    hasher.update((site_id.len() as u32).to_le_bytes());
    hasher.update(site_id.as_bytes());
    hasher.update(inhabitant_slot.to_le_bytes());
    Ok(hex_prefix(
        hasher.finalize().as_slice(),
        GENERATION_ID_BYTES,
    ))
}

/// Derive the owner-scoped opaque handle for one generation identity.
pub fn resident_handle_for_generation(
    plugin_id: &str,
    generation_id: &str,
) -> Result<String, ScriptDtoError> {
    validate_bounded_nonempty("plugin id", plugin_id, crate::MAX_PLUGIN_ID_BYTES)?;
    validate_bounded_nonempty(
        "resident generation id",
        generation_id,
        MAX_RESIDENT_GENERATION_ID_BYTES,
    )?;
    Ok(domain_handle(plugin_id, "generation", generation_id))
}

/// Derive the owner-scoped opaque handle for one adopted entity UUID.
pub fn resident_handle_for_entity(
    plugin_id: &str,
    entity_uuid: &str,
) -> Result<String, ScriptDtoError> {
    validate_bounded_nonempty("plugin id", plugin_id, crate::MAX_PLUGIN_ID_BYTES)?;
    validate_entity_uuid(entity_uuid)?;
    Ok(domain_handle(plugin_id, "entity", entity_uuid))
}

/// Derive the deterministic entity UUID for one generation identity.
///
/// Core materialises the resident exactly once per generation id because this
/// UUID is stable: re-materialising a chunk finds the existing entity instead of
/// creating a second resident.
pub fn resident_entity_uuid(generation_id: &str) -> Result<[u8; 16], ScriptDtoError> {
    validate_bounded_nonempty(
        "resident generation id",
        generation_id,
        MAX_RESIDENT_GENERATION_ID_BYTES,
    )?;
    let mut hasher = Sha256::new();
    hasher.update(b"solaris.resident.entity.v1");
    hasher.update(generation_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 variant / version 8 (custom) so the value stays a valid UUID.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(bytes)
}

/// Derive the opaque spawn site token for one owner-scoped site slot.
pub fn resident_spawn_site_token(
    plugin_id: &str,
    generation_id: &str,
) -> Result<String, ScriptDtoError> {
    validate_bounded_nonempty("plugin id", plugin_id, crate::MAX_PLUGIN_ID_BYTES)?;
    validate_bounded_nonempty(
        "resident generation id",
        generation_id,
        MAX_RESIDENT_GENERATION_ID_BYTES,
    )?;
    Ok(domain_handle(plugin_id, "spawn-site", generation_id))
}

fn domain_handle(plugin_id: &str, domain: &str, identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"solaris.resident.handle.v1");
    hasher.update(domain.as_bytes());
    hasher.update(plugin_id.as_bytes());
    hasher.update(identity.as_bytes());
    hex_prefix(hasher.finalize().as_slice(), GENERATION_ID_BYTES)
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    let mut value = String::with_capacity(length * 2);
    for byte in bytes.iter().take(length) {
        value.push_str(&format!("{byte:02x}"));
    }
    value
}

/// Validate one bounded POI handle value in a `set_resident_pois` call.
pub fn validate_resident_poi(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("resident poi", value, MAX_RESIDENT_POI_HANDLE_BYTES)
}

/// Validate one bounded opaque spawn site token.
pub fn validate_spawn_site_token(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("spawn site token", value, MAX_RESIDENT_SPAWN_TOKEN_BYTES)
}

/// Validate a generation id supplied by core-side worldgen bootstrap.
pub fn validate_generation_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_value(
        "resident generation id",
        value,
        MAX_RESIDENT_GENERATION_ID_BYTES,
    )?;
    if value.len() != GENERATION_ID_BYTES * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ScriptDtoError::InvalidId {
            field: "resident generation id",
            actual_bytes: value.len(),
        });
    }
    Ok(())
}
