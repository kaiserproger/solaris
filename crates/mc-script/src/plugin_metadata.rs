//! Plugin-deployment metadata: what one deployed package declares about itself.
//!
//! These are the runtime-independent facts of one deployment - the ones core
//! needs to open a world against it, stage its client bundles and report what it
//! contributes - owned here so a consumer reads them without a script runtime
//! linked in. Nothing here knows how a rule was evaluated.
//!
//! The component host materializes this metadata from the package it validated
//! before startup. Client bundles, worldgen profiles and settlement plans share
//! this one representation with the server's startup and Loader consumers.
//!
//! Fields are private and every read goes through an accessor. The constructors
//! take the parts a runtime already validated during discovery; they are the way
//! a runtime materializes that result, not a seal that stops anyone building a
//! descriptor without checking it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginWorldgenOreProfile {
    RealisticDeposits,
}

impl PluginWorldgenOreProfile {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::RealisticDeposits => "realistic_deposits",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginWorldgenSettlementProfile {
    PlainsVillagePrototype,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClientLoader {
    Fabric,
    NeoForge,
    Forge,
}

impl ClientLoader {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::NeoForge => "neoforge",
            Self::Forge => "forge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClientContentKind {
    Blocks,
    Items,
    Views,
    ViewActions,
    Assets,
    WorldPreviews,
    WorldSelection,
    Sounds,
}

impl ClientContentKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Items => "items",
            Self::Views => "views",
            Self::ViewActions => "view_actions",
            Self::Assets => "assets",
            Self::WorldPreviews => "world_previews",
            Self::WorldSelection => "world_selection",
            Self::Sounds => "sounds",
        }
    }

    /// The permission a bundle needs for this content kind.
    ///
    /// Validation and the client loader both ask this question, so the answer
    /// stays a property of the content kind instead of being restated per caller.
    #[must_use]
    pub const fn required_permission(self) -> ClientPermission {
        match self {
            Self::Blocks => ClientPermission::RegisterBlocks,
            Self::Items => ClientPermission::RegisterItems,
            Self::Views => ClientPermission::PresentViews,
            Self::ViewActions => ClientPermission::SendViewActions,
            Self::Assets => ClientPermission::LoadAssets,
            Self::WorldPreviews => ClientPermission::PresentWorldPreviews,
            Self::WorldSelection => ClientPermission::SendWorldSelection,
            Self::Sounds => ClientPermission::PlaySounds,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClientPermission {
    RegisterBlocks,
    RegisterItems,
    PresentViews,
    SendViewActions,
    LoadAssets,
    PresentWorldPreviews,
    SendWorldSelection,
    PlaySounds,
}

impl ClientPermission {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::RegisterBlocks => "register_blocks",
            Self::RegisterItems => "register_items",
            Self::PresentViews => "present_views",
            Self::SendViewActions => "send_view_actions",
            Self::LoadAssets => "load_assets",
            Self::PresentWorldPreviews => "present_world_previews",
            Self::SendWorldSelection => "send_world_selection",
            Self::PlaySounds => "play_sounds",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientBundle {
    owner_plugin_id: String,
    id: String,
    version: String,
    artifact: String,
    sha256: String,
    size_bytes: u64,
    artifact_path: PathBuf,
    artifact_bytes: Arc<[u8]>,
    loaders: Vec<ClientLoader>,
    content: Vec<ClientContentKind>,
    permissions: Vec<ClientPermission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginDeployment {
    ServerOnly,
    ServerAndClient,
}

impl PluginDeployment {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::ServerOnly => "server_only",
            Self::ServerAndClient => "server_and_client",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientBundleDiscovery<'a> {
    id: &'a str,
    version: &'a str,
    artifact: &'a str,
    sha256: &'a str,
    size_bytes: u64,
    loaders: Vec<&'static str>,
    content: Vec<&'static str>,
    permissions: Vec<&'static str>,
}

impl<'a> ClientBundleDiscovery<'a> {
    /// Build one already-validated discovery row for one client bundle.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &'a str,
        version: &'a str,
        artifact: &'a str,
        sha256: &'a str,
        size_bytes: u64,
        loaders: Vec<&'static str>,
        content: Vec<&'static str>,
        permissions: Vec<&'static str>,
    ) -> Self {
        Self {
            id,
            version,
            artifact,
            sha256,
            size_bytes,
            loaders,
            content,
            permissions,
        }
    }
    #[must_use]
    pub const fn id(&self) -> &'a str {
        self.id
    }

    #[must_use]
    pub const fn version(&self) -> &'a str {
        self.version
    }

    #[must_use]
    pub const fn artifact(&self) -> &'a str {
        self.artifact
    }

    #[must_use]
    pub const fn sha256(&self) -> &'a str {
        self.sha256
    }

    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[must_use]
    pub fn loaders(&self) -> &[&'static str] {
        &self.loaders
    }

    #[must_use]
    pub fn content(&self) -> &[&'static str] {
        &self.content
    }

    #[must_use]
    pub fn permissions(&self) -> &[&'static str] {
        &self.permissions
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginDiscovery<'a> {
    id: &'a str,
    deployment: PluginDeployment,
    supported_loaders: Vec<&'static str>,
    permissions: Vec<&'static str>,
    total_artifact_bytes: u64,
    client_bundles: Vec<ClientBundleDiscovery<'a>>,
}

impl<'a> PluginDiscovery<'a> {
    /// Build one already-validated discovery row for one deployed plugin.
    #[must_use]
    pub fn new(
        id: &'a str,
        deployment: PluginDeployment,
        supported_loaders: Vec<&'static str>,
        permissions: Vec<&'static str>,
        total_artifact_bytes: u64,
        client_bundles: Vec<ClientBundleDiscovery<'a>>,
    ) -> Self {
        Self {
            id,
            deployment,
            supported_loaders,
            permissions,
            total_artifact_bytes,
            client_bundles,
        }
    }
    #[must_use]
    pub const fn id(&self) -> &'a str {
        self.id
    }

    #[must_use]
    pub const fn deployment(&self) -> PluginDeployment {
        self.deployment
    }

    #[must_use]
    pub fn supported_loaders(&self) -> &[&'static str] {
        &self.supported_loaders
    }

    #[must_use]
    pub fn permissions(&self) -> &[&'static str] {
        &self.permissions
    }

    #[must_use]
    pub const fn total_artifact_bytes(&self) -> u64 {
        self.total_artifact_bytes
    }

    #[must_use]
    pub fn client_bundles(&self) -> &[ClientBundleDiscovery<'a>] {
        &self.client_bundles
    }
}

impl ClientBundle {
    /// Build one already-validated bundle descriptor.
    ///
    /// Only discovery calls this: the artifact bytes and the sha256 are the
    /// validated ones, never a guest-supplied claim.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner_plugin_id: impl Into<String>,
        id: impl Into<String>,
        version: impl Into<String>,
        artifact: impl Into<String>,
        sha256: impl Into<String>,
        size_bytes: u64,
        artifact_path: PathBuf,
        artifact_bytes: Arc<[u8]>,
        loaders: Vec<ClientLoader>,
        content: Vec<ClientContentKind>,
        permissions: Vec<ClientPermission>,
    ) -> Self {
        Self {
            owner_plugin_id: owner_plugin_id.into(),
            id: id.into(),
            version: version.into(),
            artifact: artifact.into(),
            sha256: sha256.into(),
            size_bytes,
            artifact_path,
            artifact_bytes,
            loaders,
            content,
            permissions,
        }
    }
    #[must_use]
    pub fn owner_plugin_id(&self) -> &str {
        &self.owner_plugin_id
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn artifact(&self) -> &str {
        &self.artifact
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[must_use]
    pub fn artifact_path(&self) -> &Path {
        &self.artifact_path
    }

    #[must_use]
    pub fn artifact_bytes(&self) -> &[u8] {
        &self.artifact_bytes
    }

    #[must_use]
    pub fn artifact_bytes_arc(&self) -> Arc<[u8]> {
        Arc::clone(&self.artifact_bytes)
    }

    #[must_use]
    pub fn loaders(&self) -> &[ClientLoader] {
        &self.loaders
    }

    #[must_use]
    pub fn content(&self) -> &[ClientContentKind] {
        &self.content
    }

    #[must_use]
    pub fn permissions(&self) -> &[ClientPermission] {
        &self.permissions
    }

    #[must_use]
    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}/{}/{}",
            self.owner_plugin_id, self.id, self.version, self.sha256
        )
    }
}

impl PluginWorldgenSettlementProfile {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::PlainsVillagePrototype => "plains_village_prototype",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PluginSettlementBuildingTemplate {
    PlainsFountain,
    PlainsSmallHouse,
    PlainsToolsmith,
}

impl PluginSettlementBuildingTemplate {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::PlainsFountain => "plains_fountain",
            Self::PlainsSmallHouse => "plains_small_house",
            Self::PlainsToolsmith => "plains_toolsmith",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginSettlementBuildingRole {
    MeetingPoint,
    Home,
    Workplace,
}

impl PluginSettlementBuildingRole {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::MeetingPoint => "meeting_point",
            Self::Home => "home",
            Self::Workplace => "workplace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginSettlementInhabitantKind {
    Villager,
}

impl PluginSettlementInhabitantKind {
    #[must_use]
    pub const fn entity_type(self) -> &'static str {
        match self {
            Self::Villager => "minecraft:villager",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginSettlementJob {
    Unemployed,
    Toolsmith,
}

impl PluginSettlementJob {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Unemployed => "unemployed",
            Self::Toolsmith => "toolsmith",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSettlementBuilding {
    id: String,
    template: PluginSettlementBuildingTemplate,
    role: PluginSettlementBuildingRole,
}

impl PluginSettlementBuilding {
    /// Build one already-validated settlement building.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        template: PluginSettlementBuildingTemplate,
        role: PluginSettlementBuildingRole,
    ) -> Self {
        Self {
            id: id.into(),
            template,
            role,
        }
    }
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn template(&self) -> PluginSettlementBuildingTemplate {
        self.template
    }

    #[must_use]
    pub const fn role(&self) -> PluginSettlementBuildingRole {
        self.role
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSettlementInhabitant {
    id: String,
    kind: PluginSettlementInhabitantKind,
    building_id: String,
    job: PluginSettlementJob,
}

impl PluginSettlementInhabitant {
    /// Build one already-validated settlement inhabitant.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        kind: PluginSettlementInhabitantKind,
        building_id: impl Into<String>,
        job: PluginSettlementJob,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            building_id: building_id.into(),
            job,
        }
    }
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn kind(&self) -> PluginSettlementInhabitantKind {
        self.kind
    }

    #[must_use]
    pub fn building_id(&self) -> &str {
        &self.building_id
    }

    #[must_use]
    pub const fn job(&self) -> PluginSettlementJob {
        self.job
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSettlementExtension {
    id: String,
    building_id: String,
}

impl PluginSettlementExtension {
    /// Build one already-validated settlement extension.
    #[must_use]
    pub fn new(id: impl Into<String>, building_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            building_id: building_id.into(),
        }
    }
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn building_id(&self) -> &str {
        &self.building_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSettlementPlan {
    owner_plugin_id: String,
    profile: PluginWorldgenSettlementProfile,
    buildings: Vec<PluginSettlementBuilding>,
    inhabitants: Vec<PluginSettlementInhabitant>,
    extensions: Vec<PluginSettlementExtension>,
}

impl PluginSettlementPlan {
    /// Build one already-validated settlement plan.
    #[must_use]
    pub fn new(
        owner_plugin_id: impl Into<String>,
        profile: PluginWorldgenSettlementProfile,
        buildings: Vec<PluginSettlementBuilding>,
        inhabitants: Vec<PluginSettlementInhabitant>,
        extensions: Vec<PluginSettlementExtension>,
    ) -> Self {
        Self {
            owner_plugin_id: owner_plugin_id.into(),
            profile,
            buildings,
            inhabitants,
            extensions,
        }
    }
    #[must_use]
    pub fn plains_village_prototype(owner_plugin_id: impl Into<String>) -> Self {
        Self {
            owner_plugin_id: owner_plugin_id.into(),
            profile: PluginWorldgenSettlementProfile::PlainsVillagePrototype,
            buildings: default_plains_village_buildings(),
            inhabitants: Vec::new(),
            extensions: Vec::new(),
        }
    }

    #[must_use]
    pub fn owner_plugin_id(&self) -> &str {
        &self.owner_plugin_id
    }

    #[must_use]
    pub const fn profile(&self) -> PluginWorldgenSettlementProfile {
        self.profile
    }

    #[must_use]
    pub fn buildings(&self) -> &[PluginSettlementBuilding] {
        &self.buildings
    }

    #[must_use]
    pub fn inhabitants(&self) -> &[PluginSettlementInhabitant] {
        &self.inhabitants
    }

    #[must_use]
    pub fn extensions(&self) -> &[PluginSettlementExtension] {
        &self.extensions
    }

    #[must_use]
    pub fn contract_name(&self) -> String {
        let mut contract = format!(
            "{}|owner={}|buildings=",
            self.profile.contract_name(),
            self.owner_plugin_id
        );
        for building in &self.buildings {
            contract.push_str(&format!(
                "{},{},{};",
                building.id,
                building.template.contract_name(),
                building.role.contract_name()
            ));
        }
        contract.push_str("|inhabitants=");
        for inhabitant in &self.inhabitants {
            contract.push_str(&format!(
                "{},{},{},{};",
                inhabitant.id,
                inhabitant.kind.entity_type(),
                inhabitant.building_id,
                inhabitant.job.contract_name()
            ));
        }
        contract.push_str("|extensions=");
        for extension in &self.extensions {
            contract.push_str(&format!("{},{};", extension.id, extension.building_id));
        }
        contract
    }
}
/// The buildings a plains village prototype starts from.
///
/// Shared by direct prototype construction and component packages that declare a
/// settlement profile without naming buildings. One list keeps both paths on the
/// same village layout.
pub fn default_plains_village_buildings() -> Vec<PluginSettlementBuilding> {
    [
        (
            "meeting-point",
            PluginSettlementBuildingTemplate::PlainsFountain,
            PluginSettlementBuildingRole::MeetingPoint,
        ),
        (
            "home",
            PluginSettlementBuildingTemplate::PlainsSmallHouse,
            PluginSettlementBuildingRole::Home,
        ),
        (
            "toolsmith",
            PluginSettlementBuildingTemplate::PlainsToolsmith,
            PluginSettlementBuildingRole::Workplace,
        ),
    ]
    .into_iter()
    .map(|(id, template, role)| PluginSettlementBuilding {
        id: id.to_owned(),
        template,
        role,
    })
    .collect()
}
