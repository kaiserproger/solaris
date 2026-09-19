//! Settlement site, survey, and construction DTOs.
//!
//! These types are the closed wire contract for the settlement operations of
//! the script API: site discovery pages, site queries, resident site
//! reservation, terrain surveys, and durable structure preparation,
//! advancement, and status. They are pure serde values with explicit bounds and
//! deliberately carry no registry or authoritative world access.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    MAX_SCRIPT_WORLD_TIME, ScriptDtoError, check_contract_resource_id, validate_bounded_nonempty,
    validate_generation_id, validate_spawn_site_token,
};

/// Maximum sites returned by one `list_sites` or `query_site` page.
pub const MAX_SETTLEMENT_SITE_PAGE: usize = 64;
/// Maximum buildings placed into one settlement site.
pub const MAX_SETTLEMENT_PLACEMENTS: usize = 128;
/// Maximum points of interest attached to one settlement site.
pub const MAX_SETTLEMENT_POIS: usize = 128;
/// Maximum inhabitants generated for one settlement site.
pub const MAX_SETTLEMENT_RESIDENTS: usize = 80;
/// Maximum byte length of a deterministic settlement site id.
pub const MAX_SITE_ID_BYTES: usize = 64;
/// Maximum byte length of a catalog blueprint id.
pub const MAX_BLUEPRINT_ID_BYTES: usize = 64;
/// Maximum byte length of a durable structure id.
pub const MAX_STRUCTURE_ID_BYTES: usize = 64;
/// Maximum byte length of an opaque survey token.
pub const MAX_SURVEY_TOKEN_BYTES: usize = 64;
/// Maximum extent of one survey bounds axis, in columns.
pub const MAX_SURVEY_BOUNDS_AXIS: i32 = 128;
/// Maximum biome or resource tags on one survey snapshot or column.
pub const MAX_SURVEY_TAGS: usize = 32;
/// Maximum active structures one plugin may hold.
pub const MAX_STRUCTURE_ACTIVE_PER_PLUGIN: usize = 64;
/// Maximum work units committed by one `advance_structure` portion.
pub const MAX_WORLD_COMMIT_PORTION: usize = 512;
/// Maximum distinct resources named by one structure material list.
pub const MAX_STRUCTURE_RESOURCE_TYPES: usize = 16;
/// Maximum stages one structure snapshot may plan.
pub const MAX_STRUCTURE_STAGES: usize = 32;

/// Maximum footprint extent of one catalog blueprint (one building), per axis.
///
/// This is the blueprint bound from the frozen catalog contract; a settlement
/// *site* is territory and uses [`MAX_SETTLEMENT_SITE_AXIS`] instead.
pub const MAX_BLUEPRINT_FOOTPRINT_AXIS: i32 = 64;
/// Maximum footprint extent of one settlement site (its reserved territory), per
/// axis. Site footprints are 128/192/256 by variant and are never squeezed into
/// the per-building blueprint bound.
pub const MAX_SETTLEMENT_SITE_AXIS: i32 = 256;

/// Settlement size class selected by deterministic worldgen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptSiteVariant {
    Hamlet,
    Village,
    Town,
}

impl ScriptSiteVariant {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hamlet => "hamlet",
            Self::Village => "village",
            Self::Town => "town",
        }
    }
}

/// Authored point-of-interest role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptSitePoiKind {
    Home,
    Work,
    Meeting,
    Guard,
}

impl ScriptSitePoiKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Work => "work",
            Self::Meeting => "meeting",
            Self::Guard => "guard",
        }
    }
}

/// Who authored the settlement a site describes.
///
/// A site is either one the settlement owner laid out itself from its own
/// authoring catalog, or one core generated as a vanilla village. The two carry
/// the same description; the provenance is what makes an identity an authored
/// generation id or a generated entity's UUID, so a caller never has to guess
/// which it is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptSiteProvenance {
    /// The settlement owner's own catalog laid this site out.
    Authored,
    /// Core generated this site as a vanilla village.
    VanillaVillage,
}

impl ScriptSiteProvenance {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authored => "authored",
            Self::VanillaVillage => "vanilla_village",
        }
    }
}

/// One entity identity: a canonical lowercase hyphenated UUID.
///
/// A generated village's site descriptor names the inhabitants that already
/// exist, and their generation mints entity UUIDs (`settlement_uuid` over the
/// placement's claim), so the identity a site reports for such an inhabitant is
/// that UUID and nothing else — never a position-derived stand-in.
fn validate_entity_uuid(value: &str) -> Result<(), ScriptDtoError> {
    let bytes = value.as_bytes();
    let shaped = bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
            }
        });
    if !shaped {
        return Err(ScriptDtoError::InvalidId {
            field: "inhabitant entity identity",
            actual_bytes: bytes.len(),
        });
    }
    Ok(())
}

/// Occupancy state of one settlement point of interest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptSitePoiState {
    Free,
    Reserved,
    Occupied,
}

impl ScriptSitePoiState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Reserved => "reserved",
            Self::Occupied => "occupied",
        }
    }
}

/// Why a plugin requested a survey of one bounded region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptSurveyPurpose {
    Settlement,
    Expansion,
    Restoration,
}

impl ScriptSurveyPurpose {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settlement => "settlement",
            Self::Expansion => "expansion",
            Self::Restoration => "restoration",
        }
    }
}

/// Whether the surveyed chunks are currently loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptChunkAvailability {
    Loaded,
    Unloaded,
}

impl ScriptChunkAvailability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Unloaded => "unloaded",
        }
    }
}

/// Durable lifecycle state of one structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptStructureState {
    Prepared,
    Running,
    Paused,
    Committed,
    Cancelled,
}

impl ScriptStructureState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Committed => "committed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One blueprint placement inside a settlement site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementBuilding {
    pub blueprint_id: String,
    pub origin: [i32; 3],
    pub rotation: u16,
}

impl ScriptSettlementBuilding {
    #[must_use]
    pub fn new(blueprint_id: String, origin: [i32; 3], rotation: u16) -> Self {
        Self {
            blueprint_id,
            origin,
            rotation,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_blueprint_id(&self.blueprint_id)?;
        validate_rotation(self.rotation)
    }
}

/// One point of interest resolved against a settled site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementPoi {
    pub poi_id: String,
    pub kind: ScriptSitePoiKind,
    pub at: [i32; 3],
    pub capacity: u16,
    pub state: ScriptSitePoiState,
}

impl ScriptSettlementPoi {
    #[must_use]
    pub fn new(
        poi_id: String,
        kind: ScriptSitePoiKind,
        at: [i32; 3],
        capacity: u16,
        state: ScriptSitePoiState,
    ) -> Self {
        Self {
            poi_id,
            kind,
            at,
            capacity,
            state,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_poi_id(&self.poi_id)
    }
}

/// One fully described settlement site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementSite {
    pub site_id: String,
    pub provenance: ScriptSiteProvenance,
    pub variant: ScriptSiteVariant,
    pub revision: u64,
    /// Whether the point-of-interest and inhabitant lists are the site's
    /// contents.
    ///
    /// An authored site always describes its own layout, and a generated
    /// village describes itself once core can read its chunks. Until then the
    /// two lists are empty because the contents are *unknown*, not because the
    /// village holds nothing: a caller must not read an ungenerated village as
    /// an empty one (`ACC-05`).
    pub contents_known: bool,
    pub footprint_origin: [i32; 3],
    pub footprint_size: [i32; 3],
    pub buildings: Vec<ScriptSettlementBuilding>,
    pub pois: Vec<ScriptSettlementPoi>,
    pub inhabitant_generation_ids: Vec<String>,
}

impl ScriptSettlementSite {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        site_id: String,
        provenance: ScriptSiteProvenance,
        variant: ScriptSiteVariant,
        revision: u64,
        contents_known: bool,
        footprint_origin: [i32; 3],
        footprint_size: [i32; 3],
        buildings: Vec<ScriptSettlementBuilding>,
        pois: Vec<ScriptSettlementPoi>,
        inhabitant_generation_ids: Vec<String>,
    ) -> Self {
        Self {
            site_id,
            provenance,
            variant,
            revision,
            contents_known,
            footprint_origin,
            footprint_size,
            buildings,
            pois,
            inhabitant_generation_ids,
        }
    }

    /// Order every unordered set so equal sites compare equal on the wire.
    pub fn canonicalize(&mut self) {
        self.buildings.sort_unstable_by(|left, right| {
            left.blueprint_id
                .cmp(&right.blueprint_id)
                .then_with(|| left.origin.cmp(&right.origin))
                .then_with(|| left.rotation.cmp(&right.rotation))
        });
        self.pois
            .sort_unstable_by(|left, right| left.poi_id.cmp(&right.poi_id));
        self.inhabitant_generation_ids.sort_unstable();
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_site_id(&self.site_id)?;
        validate_revision(self.revision)?;
        validate_site_footprint(self.footprint_size)?;
        if self.buildings.len() > MAX_SETTLEMENT_PLACEMENTS
            || self.pois.len() > MAX_SETTLEMENT_POIS
            || self.inhabitant_generation_ids.len() > MAX_SETTLEMENT_RESIDENTS
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        for building in &self.buildings {
            building.validate()?;
        }
        let mut poi_ids = BTreeSet::new();
        for poi in &self.pois {
            poi.validate()?;
            if !poi_ids.insert(poi.poi_id.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        let mut generation_ids = BTreeSet::new();
        for generation_id in &self.inhabitant_generation_ids {
            match self.provenance {
                ScriptSiteProvenance::Authored => validate_generation_id(generation_id)?,
                // A generated village's inhabitants already exist, so the site
                // names their entity identities: the UUIDs the generator's own
                // placements minted, and the ids `claim_resident` takes.
                ScriptSiteProvenance::VanillaVillage => validate_entity_uuid(generation_id)?,
            }
            if !generation_ids.insert(generation_id.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}

/// One bounded page of settlement sites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementSitePage {
    pub sites: Vec<ScriptSettlementSite>,
    pub cursor: Option<String>,
}

impl ScriptSettlementSitePage {
    #[must_use]
    pub fn new(sites: Vec<ScriptSettlementSite>, cursor: Option<String>) -> Self {
        Self { sites, cursor }
    }

    /// Order the page by site id so pagination is reproducible.
    pub fn canonicalize(&mut self) {
        self.sites
            .sort_unstable_by(|left, right| left.site_id.cmp(&right.site_id));
        for site in &mut self.sites {
            site.canonicalize();
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self.sites.len() > MAX_SETTLEMENT_SITE_PAGE {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(cursor) = &self.cursor {
            validate_cursor(cursor)?;
        }
        let mut site_ids = BTreeSet::new();
        for site in &self.sites {
            site.validate()?;
            if !site_ids.insert(site.site_id.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}

/// Inclusive axis-aligned survey bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSurveyBounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl ScriptSurveyBounds {
    pub fn new(min: [i32; 3], max: [i32; 3]) -> Result<Self, ScriptDtoError> {
        let bounds = Self { min, max };
        bounds.validate()?;
        Ok(bounds)
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        for (min, max) in self.min.iter().zip(self.max.iter()) {
            let extent = i64::from(*max) - i64::from(*min) + 1;
            if extent <= 0 || extent > i64::from(MAX_SURVEY_BOUNDS_AXIS) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }

    /// Number of surface columns covered by the horizontal extents.
    #[must_use]
    pub fn columns(&self) -> u64 {
        let x = i64::from(self.max[0]) - i64::from(self.min[0]) + 1;
        let z = i64::from(self.max[2]) - i64::from(self.min[2]) + 1;
        (x * z) as u64
    }

    #[must_use]
    pub fn min(&self) -> [i32; 3] {
        self.min
    }

    #[must_use]
    pub fn max(&self) -> [i32; 3] {
        self.max
    }
}

/// One bounded, core-owned terrain survey snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSurveySnapshot {
    pub dimension: String,
    pub bounds: ScriptSurveyBounds,
    pub revision: u64,
    pub chunk_availability: ScriptChunkAvailability,
    pub survey_token: String,
    pub usable_plots: u32,
    pub water_columns: u32,
    pub claimed: bool,
    pub existing_structures: u32,
    pub biome_tags: Vec<String>,
    pub resource_tags: Vec<String>,
}

impl ScriptSurveySnapshot {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dimension: String,
        bounds: ScriptSurveyBounds,
        revision: u64,
        chunk_availability: ScriptChunkAvailability,
        survey_token: String,
        usable_plots: u32,
        water_columns: u32,
        claimed: bool,
        existing_structures: u32,
        biome_tags: Vec<String>,
        resource_tags: Vec<String>,
    ) -> Self {
        Self {
            dimension,
            bounds,
            revision,
            chunk_availability,
            survey_token,
            usable_plots,
            water_columns,
            claimed,
            existing_structures,
            biome_tags,
            resource_tags,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        check_contract_resource_id(&self.dimension)?;
        self.bounds.validate()?;
        validate_revision(self.revision)?;
        validate_bounded_nonempty("survey token", &self.survey_token, MAX_SURVEY_TOKEN_BYTES)?;
        // Every column the survey read is either a usable plot or water, so the
        // aggregates partition the surveyed tile. The snapshot stays bounded: it
        // carries the aggregate reading, never one record per column, because a
        // 128x128 tile cannot be materialized inside the handler
        // instruction/wall budget (contract 11.4).
        let columns = self.bounds.columns();
        if u64::from(self.usable_plots) + u64::from(self.water_columns) > columns {
            return Err(ScriptDtoError::InvalidBounds);
        }
        validate_tags("survey biome tags", &self.biome_tags)?;
        validate_tags("survey resource tags", &self.resource_tags)
    }
}

/// One durable resident reservation against a settled site point of interest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptResidentSiteReservation {
    pub site_id: String,
    pub poi_id: String,
    pub spawn_site_token: String,
    pub revision: u64,
}

impl ScriptResidentSiteReservation {
    #[must_use]
    pub fn new(site_id: String, poi_id: String, spawn_site_token: String, revision: u64) -> Self {
        Self {
            site_id,
            poi_id,
            spawn_site_token,
            revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_site_id(&self.site_id)?;
        validate_poi_id(&self.poi_id)?;
        validate_spawn_site_token(&self.spawn_site_token)?;
        validate_revision(self.revision)
    }
}

/// One authored stage plan for a structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureStagePlan {
    pub stage: String,
    pub block_count: u32,
    pub work_units: u64,
    pub materials: Vec<ScriptStructureMaterial>,
}

impl ScriptStructureStagePlan {
    #[must_use]
    pub fn new(
        stage: String,
        block_count: u32,
        work_units: u64,
        materials: Vec<ScriptStructureMaterial>,
    ) -> Self {
        Self {
            stage,
            block_count,
            work_units,
            materials,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_stage(&self.stage)?;
        validate_work_units(self.work_units)?;
        validate_materials(&self.materials)
    }
}

/// One bounded resource requirement of a structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureMaterial {
    pub resource: String,
    pub quantity: u64,
}

impl ScriptStructureMaterial {
    #[must_use]
    pub fn new(resource: String, quantity: u64) -> Self {
        Self { resource, quantity }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        check_contract_resource_id(&self.resource)?;
        if self.quantity == 0 || self.quantity > MAX_SCRIPT_WORLD_TIME {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// One committed portion receipt of a structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureReceipt {
    pub structure_id: String,
    pub stage: String,
    pub sequence: u64,
    pub block_count: u32,
    pub work_units: u64,
    pub consumed: Vec<ScriptStructureMaterial>,
    pub revision: u64,
}

impl ScriptStructureReceipt {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        structure_id: String,
        stage: String,
        sequence: u64,
        block_count: u32,
        work_units: u64,
        consumed: Vec<ScriptStructureMaterial>,
        revision: u64,
    ) -> Self {
        Self {
            structure_id,
            stage,
            sequence,
            block_count,
            work_units,
            consumed,
            revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_structure_id(&self.structure_id)?;
        validate_stage(&self.stage)?;
        validate_revision(self.sequence)?;
        validate_commit_portion(self.work_units)?;
        validate_materials(&self.consumed)?;
        validate_revision(self.revision)
    }
}

/// One bounded, core-owned structure snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureSnapshot {
    pub structure_id: String,
    pub blueprint_id: String,
    pub site_id: String,
    pub state: ScriptStructureState,
    pub revision: u64,
    pub origin: [i32; 3],
    pub rotation: u16,
    pub reserved_footprint: [i32; 3],
    pub stages: Vec<ScriptStructureStagePlan>,
    pub resource_plan_hash: String,
    pub reservation_ref: Option<String>,
    pub watermark: u64,
    pub consumed: Vec<ScriptStructureMaterial>,
    pub remaining: Vec<ScriptStructureMaterial>,
    pub pause_reason: Option<String>,
}

impl ScriptStructureSnapshot {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        structure_id: String,
        blueprint_id: String,
        site_id: String,
        state: ScriptStructureState,
        revision: u64,
        origin: [i32; 3],
        rotation: u16,
        reserved_footprint: [i32; 3],
        stages: Vec<ScriptStructureStagePlan>,
        resource_plan_hash: String,
        reservation_ref: Option<String>,
        watermark: u64,
        consumed: Vec<ScriptStructureMaterial>,
        remaining: Vec<ScriptStructureMaterial>,
        pause_reason: Option<String>,
    ) -> Self {
        Self {
            structure_id,
            blueprint_id,
            site_id,
            state,
            revision,
            origin,
            rotation,
            reserved_footprint,
            stages,
            resource_plan_hash,
            reservation_ref,
            watermark,
            consumed,
            remaining,
            pause_reason,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_structure_id(&self.structure_id)?;
        validate_blueprint_id(&self.blueprint_id)?;
        validate_site_id(&self.site_id)?;
        validate_revision(self.revision)?;
        validate_rotation(self.rotation)?;
        validate_blueprint_footprint(self.reserved_footprint)?;
        if self.stages.len() > MAX_STRUCTURE_STAGES {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut stages = BTreeSet::new();
        for stage in &self.stages {
            stage.validate()?;
            if !stages.insert(stage.stage.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        validate_hex_hash("structure resource plan hash", &self.resource_plan_hash)?;
        if let Some(reservation_ref) = &self.reservation_ref {
            validate_reservation_ref(reservation_ref)?;
        }
        validate_revision(self.watermark)?;
        validate_materials(&self.consumed)?;
        validate_materials(&self.remaining)?;
        if let Some(pause_reason) = &self.pause_reason {
            validate_bounded_nonempty("structure pause reason", pause_reason, MAX_SITE_ID_BYTES)?;
        }
        Ok(())
    }
}

/// The core-authenticated origin of one warehouse binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptWarehouseSource {
    Authored {
        structure_id: String,
        container_id: u32,
    },
    VanillaVillage {
        site_id: String,
        container_id: u32,
    },
}

impl ScriptWarehouseSource {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Authored { structure_id, .. } => validate_structure_id(structure_id),
            Self::VanillaVillage { site_id, .. } => validate_site_id(site_id),
        }
    }
}

/// One core-issued warehouse binding for an authored structure container or a
/// materialized generator-authenticated village container.
///
/// `handle` is opaque to the plugin: the plugin never parses it and never
/// chooses a container by coordinates. The source ordinal is resolved by core;
/// a village binding retains its exact source position durably outside this
/// public snapshot so the ordinal is never reinterpreted after the bind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedWarehouseBinding")]
#[non_exhaustive]
pub struct ScriptWarehouseBinding {
    pub handle: String,
    pub source: ScriptWarehouseSource,
    pub revision: u64,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum UncheckedWarehouseBinding {
    Current {
        handle: String,
        source: ScriptWarehouseSource,
        revision: u64,
    },
    LegacyAuthored {
        handle: String,
        structure_id: String,
        container_id: u32,
        revision: u64,
    },
}

impl TryFrom<UncheckedWarehouseBinding> for ScriptWarehouseBinding {
    type Error = ScriptDtoError;

    fn try_from(value: UncheckedWarehouseBinding) -> Result<Self, Self::Error> {
        let binding = match value {
            UncheckedWarehouseBinding::Current {
                handle,
                source,
                revision,
            } => Self::new(handle, source, revision),
            UncheckedWarehouseBinding::LegacyAuthored {
                handle,
                structure_id,
                container_id,
                revision,
            } => Self::new(
                handle,
                ScriptWarehouseSource::Authored {
                    structure_id,
                    container_id,
                },
                revision,
            ),
        };
        binding.validate()?;
        Ok(binding)
    }
}

impl ScriptWarehouseBinding {
    #[must_use]
    pub fn new(handle: String, source: ScriptWarehouseSource, revision: u64) -> Self {
        Self {
            handle,
            source,
            revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty(
            "warehouse handle",
            &self.handle,
            crate::MAX_WAREHOUSE_HANDLE_BYTES,
        )?;
        self.source.validate()?;
        validate_revision(self.revision)
    }
}

/// Derive the core-issued opaque handle for one authored container.
///
/// The handle is owner-scoped and embeds the durable structure and authored
/// container identity, so it can never collide across plugins. It is minted by
/// core only: a plugin supplies a container ordinal, never a handle.
pub fn warehouse_handle(
    plugin_id: &str,
    structure_id: &str,
    container_id: u32,
) -> Result<String, ScriptDtoError> {
    validate_bounded_nonempty("plugin id", plugin_id, crate::MAX_PLUGIN_ID_BYTES)?;
    validate_structure_id(structure_id)?;
    let handle = format!("warehouse:{plugin_id}:{structure_id}:{container_id}");
    validate_bounded_nonempty(
        "warehouse handle",
        &handle,
        crate::MAX_WAREHOUSE_HANDLE_BYTES,
    )?;
    Ok(handle)
}

/// One bounded settlement call inside the durable operation envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptSettlementOperation {
    ListSites {
        cursor: Option<String>,
        limit: u8,
    },
    QuerySite {
        site_id: String,
        cursor: Option<String>,
        limit: u8,
    },
    ReserveResidentSite {
        operation_id: String,
        site_id: String,
        poi_id: String,
        expected_site_revision: u64,
    },
    ReleaseResidentSite {
        operation_id: String,
        spawn_site_token: String,
    },
    Survey {
        dimension: String,
        bounds: ScriptSurveyBounds,
        purpose: ScriptSurveyPurpose,
    },
    PrepareStructure {
        operation_id: String,
        blueprint_id: String,
        anchor: [i32; 3],
        rotation: u16,
        survey_token: String,
        expected_site_revision: u64,
    },
    AdvanceStructure {
        operation_id: String,
        structure_id: String,
        stage: String,
        reservation_ref: String,
        expected_revision: u64,
        work_units: u64,
    },
    PauseStructure {
        operation_id: String,
        structure_id: String,
        expected_revision: u64,
    },
    ResumeStructure {
        operation_id: String,
        structure_id: String,
        expected_revision: u64,
    },
    CancelStructure {
        operation_id: String,
        structure_id: String,
        expected_revision: u64,
    },
    Status {
        structure_id: String,
    },
    BindWarehouse {
        operation_id: String,
        structure_id: String,
        container_id: u32,
    },
    BindVillageWarehouse {
        operation_id: String,
        site_id: String,
        container_id: u32,
    },
}

impl ScriptSettlementOperation {
    /// Longest byte length accepted for a structure reservation reference.
    const MAX_RESERVATION_REF_BYTES: usize = 64;

    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::ReserveResidentSite { operation_id, .. }
            | Self::ReleaseResidentSite { operation_id, .. }
            | Self::PrepareStructure { operation_id, .. }
            | Self::AdvanceStructure { operation_id, .. }
            | Self::PauseStructure { operation_id, .. }
            | Self::ResumeStructure { operation_id, .. }
            | Self::CancelStructure { operation_id, .. }
            | Self::BindWarehouse { operation_id, .. }
            | Self::BindVillageWarehouse { operation_id, .. } => Some(operation_id),
            Self::ListSites { .. }
            | Self::QuerySite { .. }
            | Self::Survey { .. }
            | Self::Status { .. } => None,
        }
    }

    /// No operation variant carries an unordered collection: site snapshots are
    /// canonicalised by [`ScriptSettlementSite::canonicalize`] and pages by
    /// [`ScriptSettlementSitePage::canonicalize`].
    pub fn canonicalize(&mut self) {}

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(operation_id) = self.operation_id() {
            crate::validate_script_id_value(operation_id)?;
        }
        match self {
            Self::ListSites { cursor, limit } => {
                validate_limit(*limit)?;
                if let Some(cursor) = cursor {
                    validate_cursor(cursor)?;
                }
            }
            Self::QuerySite {
                site_id,
                cursor,
                limit,
            } => {
                validate_site_id(site_id)?;
                validate_limit(*limit)?;
                if let Some(cursor) = cursor {
                    validate_cursor(cursor)?;
                }
            }
            Self::ReserveResidentSite {
                site_id,
                poi_id,
                expected_site_revision,
                ..
            } => {
                validate_site_id(site_id)?;
                validate_poi_id(poi_id)?;
                validate_revision(*expected_site_revision)?;
            }
            Self::ReleaseResidentSite {
                spawn_site_token, ..
            } => validate_spawn_site_token(spawn_site_token)?,
            Self::Survey {
                dimension, bounds, ..
            } => {
                check_contract_resource_id(dimension)?;
                bounds.validate()?;
            }
            Self::PrepareStructure {
                blueprint_id,
                rotation,
                survey_token,
                expected_site_revision,
                ..
            } => {
                validate_blueprint_id(blueprint_id)?;
                validate_rotation(*rotation)?;
                validate_bounded_nonempty("survey token", survey_token, MAX_SURVEY_TOKEN_BYTES)?;
                validate_revision(*expected_site_revision)?;
            }
            Self::AdvanceStructure {
                structure_id,
                stage,
                reservation_ref,
                expected_revision,
                work_units,
                ..
            } => {
                validate_structure_id(structure_id)?;
                validate_stage(stage)?;
                validate_reservation_ref(reservation_ref)?;
                validate_revision(*expected_revision)?;
                validate_commit_portion(*work_units)?;
            }
            Self::PauseStructure {
                structure_id,
                expected_revision,
                ..
            }
            | Self::ResumeStructure {
                structure_id,
                expected_revision,
                ..
            }
            | Self::CancelStructure {
                structure_id,
                expected_revision,
                ..
            } => {
                validate_structure_id(structure_id)?;
                validate_revision(*expected_revision)?;
            }
            Self::Status { structure_id } => validate_structure_id(structure_id)?,
            Self::BindWarehouse { structure_id, .. } => validate_structure_id(structure_id)?,
            Self::BindVillageWarehouse { site_id, .. } => validate_site_id(site_id)?,
        }
        Ok(())
    }
}

/// Closed typed payload for one committed settlement call or query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptSettlementResult {
    Sites {
        page: Box<ScriptSettlementSitePage>,
    },
    Site {
        site: Box<ScriptSettlementSite>,
    },
    ResidentSite {
        reservation: Box<ScriptResidentSiteReservation>,
    },
    Survey {
        survey: Box<ScriptSurveySnapshot>,
    },
    Structure {
        structure: Box<ScriptStructureSnapshot>,
    },
    Receipt {
        receipt: Box<ScriptStructureReceipt>,
    },
    Warehouse {
        binding: Box<ScriptWarehouseBinding>,
    },
}

impl ScriptSettlementResult {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Sites { page } => page.validate(),
            Self::Site { site } => site.validate(),
            Self::ResidentSite { reservation } => reservation.validate(),
            Self::Survey { survey } => survey.validate(),
            Self::Structure { structure } => structure.validate(),
            Self::Receipt { receipt } => receipt.validate(),
            Self::Warehouse { binding } => binding.validate(),
        }
    }
}

fn validate_limit(limit: u8) -> Result<(), ScriptDtoError> {
    if limit == 0 || usize::from(limit) > MAX_SETTLEMENT_SITE_PAGE {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_rotation(rotation: u16) -> Result<(), ScriptDtoError> {
    if matches!(rotation, 0 | 90 | 180 | 270) {
        Ok(())
    } else {
        Err(ScriptDtoError::InvalidBounds)
    }
}

fn validate_revision(revision: u64) -> Result<(), ScriptDtoError> {
    if revision > MAX_SCRIPT_WORLD_TIME {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_work_units(work_units: u64) -> Result<(), ScriptDtoError> {
    if work_units == 0 || work_units > MAX_SCRIPT_WORLD_TIME {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_commit_portion(work_units: u64) -> Result<(), ScriptDtoError> {
    if work_units == 0 || work_units > MAX_WORLD_COMMIT_PORTION as u64 {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_blueprint_footprint(size: [i32; 3]) -> Result<(), ScriptDtoError> {
    validate_axis_bounded(size, MAX_BLUEPRINT_FOOTPRINT_AXIS)
}

fn validate_site_footprint(size: [i32; 3]) -> Result<(), ScriptDtoError> {
    validate_axis_bounded(size, MAX_SETTLEMENT_SITE_AXIS)
}

fn validate_axis_bounded(size: [i32; 3], max: i32) -> Result<(), ScriptDtoError> {
    if size.iter().any(|axis| *axis <= 0 || *axis > max) {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_hex_hash(field: &'static str, value: &str) -> Result<(), ScriptDtoError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ScriptDtoError::InvalidId {
            field,
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_materials(materials: &[ScriptStructureMaterial]) -> Result<(), ScriptDtoError> {
    if materials.len() > MAX_STRUCTURE_RESOURCE_TYPES {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let mut resources = BTreeSet::new();
    for material in materials {
        material.validate()?;
        if !resources.insert(material.resource.as_str()) {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}

fn validate_tags(field: &'static str, tags: &[String]) -> Result<(), ScriptDtoError> {
    if tags.len() > MAX_SURVEY_TAGS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    for tag in tags {
        validate_bounded_nonempty(field, tag, MAX_SURVEY_TOKEN_BYTES)?;
    }
    Ok(())
}

fn validate_site_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("settlement site id", value, MAX_SITE_ID_BYTES)
}

fn validate_poi_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("settlement poi id", value, MAX_SITE_ID_BYTES)
}

fn validate_blueprint_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("settlement blueprint id", value, MAX_BLUEPRINT_ID_BYTES)?;
    check_contract_resource_id(value)
}

fn validate_structure_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("structure id", value, MAX_STRUCTURE_ID_BYTES)
}

fn validate_stage(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("structure stage", value, MAX_STRUCTURE_ID_BYTES)
}

fn validate_cursor(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("settlement cursor", value, MAX_SITE_ID_BYTES)
}

fn validate_reservation_ref(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty(
        "structure reservation",
        value,
        ScriptSettlementOperation::MAX_RESERVATION_REF_BYTES,
    )
}
