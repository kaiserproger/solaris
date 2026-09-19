//! The `[worldgen]` section of `plugin.toml`, for every package the host loads.
//!
//! A package opts into a world generation profile by declaring it here: the ore
//! profile the terrain generator deposits from, and the settlement profile with
//! its authored descriptors. The section is converted into `mc_script`'s
//! canonical component-deployment metadata, so the server has one vocabulary
//! for a package's startup contribution. An absent section contributes nothing;
//! a section that declares no profile at all is refused rather than silently
//! meaning "vanilla", and so is any descriptor that does not fit the contract's
//! own bounds.

use std::collections::HashSet;

use mc_script::{
    PluginSettlementBuilding, PluginSettlementBuildingRole, PluginSettlementBuildingTemplate,
    PluginSettlementExtension, PluginSettlementInhabitant, PluginSettlementInhabitantKind,
    PluginSettlementJob, PluginSettlementPlan, PluginWorldgenOreProfile,
    PluginWorldgenSettlementProfile, default_plains_village_buildings,
};
use serde::Deserialize;

/// Settlement buildings one plan may author.
const MAX_SETTLEMENT_BUILDINGS: usize = 3;
/// Settlement inhabitants one plan may author.
pub const MAX_SETTLEMENT_INHABITANTS: usize = 16;
/// Settlement extensions one plan may author.
const MAX_SETTLEMENT_EXTENSIONS: usize = 16;
/// Longest accepted settlement descriptor id.
const MAX_SETTLEMENT_DESCRIPTOR_ID_BYTES: usize = 48;

/// The `[worldgen]` section of a package manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskWorldgen {
    #[serde(default)]
    pub ore_profile: Option<DiskWorldgenOreProfile>,
    #[serde(default)]
    pub settlement_profile: Option<DiskWorldgenSettlementProfile>,
    #[serde(default)]
    pub settlement_buildings: Vec<DiskSettlementBuilding>,
    #[serde(default)]
    pub settlement_inhabitants: Vec<DiskSettlementInhabitant>,
    #[serde(default)]
    pub settlement_extensions: Vec<DiskSettlementExtension>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskWorldgenOreProfile {
    RealisticDeposits,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskWorldgenSettlementProfile {
    PlainsVillagePrototype,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskSettlementBuilding {
    id: String,
    template: DiskSettlementBuildingTemplate,
    role: DiskSettlementBuildingRole,
}

#[derive(Debug, Clone, Deserialize)]
enum DiskSettlementBuildingTemplate {
    #[serde(rename = "plains_fountain")]
    Fountain,
    #[serde(rename = "plains_small_house")]
    SmallHouse,
    #[serde(rename = "plains_toolsmith")]
    Toolsmith,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskSettlementBuildingRole {
    MeetingPoint,
    Home,
    Workplace,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskSettlementInhabitant {
    id: String,
    kind: DiskSettlementInhabitantKind,
    building: String,
    job: DiskSettlementJob,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskSettlementInhabitantKind {
    Villager,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskSettlementJob {
    Unemployed,
    Toolsmith,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskSettlementExtension {
    id: String,
    building: String,
}

/// One package's canonical world generation declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageWorldgen {
    ore_profile: Option<PluginWorldgenOreProfile>,
    settlement_plan: Option<PluginSettlementPlan>,
}

impl PackageWorldgen {
    /// The ore profile this package declares, if any.
    #[must_use]
    pub const fn ore_profile(&self) -> Option<PluginWorldgenOreProfile> {
        self.ore_profile
    }

    /// The settlement plan this package declares, if any.
    #[must_use]
    pub fn settlement_plan(&self) -> Option<&PluginSettlementPlan> {
        self.settlement_plan.as_ref()
    }
}

/// Convert one package's authored `[worldgen]` section into canonical values.
///
/// The plan's owner is the package's own validated plugin id, which is what names
/// its extensions in the world.
pub fn materialize_worldgen(
    owner_plugin_id: &str,
    worldgen: Option<DiskWorldgen>,
) -> Result<PackageWorldgen, String> {
    let Some(worldgen) = worldgen else {
        return Ok(PackageWorldgen::default());
    };
    if worldgen.ore_profile.is_none()
        && worldgen.settlement_profile.is_none()
        && worldgen.settlement_buildings.is_empty()
        && worldgen.settlement_inhabitants.is_empty()
        && worldgen.settlement_extensions.is_empty()
    {
        return Err("worldgen must declare ore_profile or settlement_profile".to_owned());
    }
    let ore_profile = worldgen.ore_profile.map(|profile| match profile {
        DiskWorldgenOreProfile::RealisticDeposits => PluginWorldgenOreProfile::RealisticDeposits,
    });
    let settlement_profile = worldgen.settlement_profile.map(|profile| match profile {
        DiskWorldgenSettlementProfile::PlainsVillagePrototype => {
            PluginWorldgenSettlementProfile::PlainsVillagePrototype
        }
    });
    let settlement_plan = materialize_settlement_plan(
        owner_plugin_id,
        settlement_profile,
        worldgen.settlement_buildings,
        worldgen.settlement_inhabitants,
        worldgen.settlement_extensions,
    )?;
    Ok(PackageWorldgen {
        ore_profile,
        settlement_plan,
    })
}

fn materialize_settlement_plan(
    owner_plugin_id: &str,
    profile: Option<PluginWorldgenSettlementProfile>,
    buildings: Vec<DiskSettlementBuilding>,
    inhabitants: Vec<DiskSettlementInhabitant>,
    extensions: Vec<DiskSettlementExtension>,
) -> Result<Option<PluginSettlementPlan>, String> {
    let Some(profile) = profile else {
        if buildings.is_empty() && inhabitants.is_empty() && extensions.is_empty() {
            return Ok(None);
        }
        return Err("settlement descriptors require settlement_profile".to_owned());
    };
    if buildings.len() > MAX_SETTLEMENT_BUILDINGS {
        return Err(format!(
            "settlement_buildings exceeds {MAX_SETTLEMENT_BUILDINGS} entries"
        ));
    }
    if inhabitants.len() > MAX_SETTLEMENT_INHABITANTS {
        return Err(format!(
            "settlement_inhabitants exceeds {MAX_SETTLEMENT_INHABITANTS} entries"
        ));
    }
    if extensions.len() > MAX_SETTLEMENT_EXTENSIONS {
        return Err(format!(
            "settlement_extensions exceeds {MAX_SETTLEMENT_EXTENSIONS} entries"
        ));
    }

    let buildings = if buildings.is_empty() {
        default_plains_village_buildings()
    } else {
        let mut ids = HashSet::new();
        let mut templates = HashSet::new();
        let mut materialized = Vec::with_capacity(buildings.len());
        for building in buildings {
            validate_settlement_descriptor_id(&building.id, "settlement building id")?;
            if !ids.insert(building.id.clone()) {
                return Err(format!(
                    "duplicate settlement building id {:?}",
                    building.id
                ));
            }
            let template = match building.template {
                DiskSettlementBuildingTemplate::Fountain => {
                    PluginSettlementBuildingTemplate::PlainsFountain
                }
                DiskSettlementBuildingTemplate::SmallHouse => {
                    PluginSettlementBuildingTemplate::PlainsSmallHouse
                }
                DiskSettlementBuildingTemplate::Toolsmith => {
                    PluginSettlementBuildingTemplate::PlainsToolsmith
                }
            };
            if !templates.insert(template) {
                return Err(format!(
                    "duplicate settlement building template {:?}",
                    template.contract_name()
                ));
            }
            let role = match building.role {
                DiskSettlementBuildingRole::MeetingPoint => {
                    PluginSettlementBuildingRole::MeetingPoint
                }
                DiskSettlementBuildingRole::Home => PluginSettlementBuildingRole::Home,
                DiskSettlementBuildingRole::Workplace => PluginSettlementBuildingRole::Workplace,
            };
            materialized.push(PluginSettlementBuilding::new(building.id, template, role));
        }
        materialized
    };
    let building_ids = buildings
        .iter()
        .map(|building| building.id())
        .collect::<HashSet<_>>();

    let mut inhabitant_ids = HashSet::new();
    let mut materialized_inhabitants = Vec::with_capacity(inhabitants.len());
    for inhabitant in inhabitants {
        validate_settlement_descriptor_id(&inhabitant.id, "settlement inhabitant id")?;
        if !inhabitant_ids.insert(inhabitant.id.clone()) {
            return Err(format!(
                "duplicate settlement inhabitant id {:?}",
                inhabitant.id
            ));
        }
        if !building_ids.contains(inhabitant.building.as_str()) {
            return Err(format!(
                "settlement inhabitant {:?} references unknown building {:?}",
                inhabitant.id, inhabitant.building
            ));
        }
        let kind = match inhabitant.kind {
            DiskSettlementInhabitantKind::Villager => PluginSettlementInhabitantKind::Villager,
        };
        let job = match inhabitant.job {
            DiskSettlementJob::Unemployed => PluginSettlementJob::Unemployed,
            DiskSettlementJob::Toolsmith => PluginSettlementJob::Toolsmith,
        };
        materialized_inhabitants.push(PluginSettlementInhabitant::new(
            inhabitant.id,
            kind,
            inhabitant.building,
            job,
        ));
    }

    let mut extension_ids = HashSet::new();
    let mut materialized_extensions = Vec::with_capacity(extensions.len());
    for extension in extensions {
        validate_settlement_descriptor_id(&extension.id, "settlement extension id")?;
        if !extension_ids.insert(extension.id.clone()) {
            return Err(format!(
                "duplicate settlement extension id {:?}",
                extension.id
            ));
        }
        if !building_ids.contains(extension.building.as_str()) {
            return Err(format!(
                "settlement extension {:?} references unknown building {:?}",
                extension.id, extension.building
            ));
        }
        materialized_extensions.push(PluginSettlementExtension::new(
            format!("{owner_plugin_id}:{}", extension.id),
            extension.building,
        ));
    }

    Ok(Some(PluginSettlementPlan::new(
        owner_plugin_id,
        profile,
        buildings,
        materialized_inhabitants,
        materialized_extensions,
    )))
}

fn validate_settlement_descriptor_id(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_SETTLEMENT_DESCRIPTOR_ID_BYTES {
        return Err(format!(
            "{field} must contain 1..={MAX_SETTLEMENT_DESCRIPTOR_ID_BYTES} bytes"
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.-".contains(&byte))
    {
        return Err(format!("{field} {value:?} contains invalid characters"));
    }
    Ok(())
}
