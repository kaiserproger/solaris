use super::*;

/// Host capability required by privileged outbound script commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptCommandCapability {
    SpawnEntityType { entity_type: String },
    EntityDamage,
    PluginStorage,
    StorageBatches,
    InventoryTransfers,
    PersistentResidents,
    ResidentWork,
    ResidentOrders,
    WorldSites,
    StructureOperations,
    InventoryMenus,
    InventoryStorageTransactions,
    PlayerInventory,
    Zones,
    PlayerTeleport,
    PlayerQueries,
    WorldTime,
    WorldBlocks,
    CustomPayloadChannel { channel: String },
}

/// Stable non-owning category used in public command-admission errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptCommandCapabilityKind {
    SpawnEntity,
    EntityDamage,
    PluginStorage,
    StorageBatches,
    InventoryTransfers,
    PersistentResidents,
    ResidentWork,
    ResidentOrders,
    WorldSites,
    StructureOperations,
    InventoryMenus,
    InventoryStorageTransactions,
    PlayerInventory,
    Zones,
    PlayerTeleport,
    PlayerQueries,
    WorldTime,
    WorldBlocks,
    CustomPayloadChannel,
}

impl ScriptCommandCapabilityKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::SpawnEntity => "spawn_entity",
            Self::EntityDamage => "entity_damage",
            Self::PluginStorage => "plugin_storage",
            Self::StorageBatches => "storage_batches",
            Self::InventoryTransfers => "inventory_transfers",
            Self::PersistentResidents => "persistent_residents",
            Self::ResidentWork => "resident_work",
            Self::ResidentOrders => "resident_orders",
            Self::WorldSites => "world_sites",
            Self::StructureOperations => "structure_operations",
            Self::InventoryMenus => "inventory_menus",
            Self::InventoryStorageTransactions => "inventory_storage_transactions",
            Self::PlayerInventory => "player_inventory",
            Self::Zones => "zones",
            Self::PlayerTeleport => "player_teleport",
            Self::PlayerQueries => "player_queries",
            Self::WorldTime => "world_time",
            Self::WorldBlocks => "world_blocks",
            Self::CustomPayloadChannel => "custom_payload",
        }
    }

    pub const fn field(self) -> &'static str {
        match self {
            Self::SpawnEntity => "spawn entity type",
            Self::EntityDamage => "entity damage",
            Self::PluginStorage => "plugin storage",
            Self::StorageBatches => "storage batches",
            Self::InventoryTransfers => "inventory transfer",
            Self::PersistentResidents => "persistent resident",
            Self::ResidentWork => "resident work order",
            Self::ResidentOrders => "resident squad order",
            Self::WorldSites => "settlement site",
            Self::StructureOperations => "structure operation",
            Self::InventoryMenus => "inventory menu",
            Self::InventoryStorageTransactions => "inventory storage transaction",
            Self::PlayerInventory => "player inventory transaction",
            Self::Zones => "zone",
            Self::PlayerTeleport => "player teleport",
            Self::PlayerQueries => "player query",
            Self::WorldTime => "world time",
            Self::WorldBlocks => "world block mutation",
            Self::CustomPayloadChannel => "custom payload channel",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredCommandCapability<'a> {
    SpawnEntityType { entity_type: &'a str },
    EntityDamage,
    PluginStorage,
    StorageBatches,
    InventoryTransfers,
    PersistentResidents,
    ResidentWork,
    ResidentOrders,
    WorldSites,
    StructureOperations,
    InventoryMenus,
    InventoryStorageTransactions,
    PlayerInventory,
    Zones,
    PlayerTeleport,
    PlayerQueries,
    WorldTime,
    WorldBlocks,
    CustomPayloadChannel { channel: &'a str },
}

impl RequiredCommandCapability<'_> {
    pub(super) const fn kind(self) -> ScriptCommandCapabilityKind {
        match self {
            Self::SpawnEntityType { .. } => ScriptCommandCapabilityKind::SpawnEntity,
            Self::EntityDamage => ScriptCommandCapabilityKind::EntityDamage,
            Self::PluginStorage => ScriptCommandCapabilityKind::PluginStorage,
            Self::StorageBatches => ScriptCommandCapabilityKind::StorageBatches,
            Self::InventoryTransfers => ScriptCommandCapabilityKind::InventoryTransfers,
            Self::PersistentResidents => ScriptCommandCapabilityKind::PersistentResidents,
            Self::ResidentWork => ScriptCommandCapabilityKind::ResidentWork,
            Self::ResidentOrders => ScriptCommandCapabilityKind::ResidentOrders,
            Self::WorldSites => ScriptCommandCapabilityKind::WorldSites,
            Self::StructureOperations => ScriptCommandCapabilityKind::StructureOperations,
            Self::InventoryMenus => ScriptCommandCapabilityKind::InventoryMenus,
            Self::InventoryStorageTransactions => {
                ScriptCommandCapabilityKind::InventoryStorageTransactions
            }
            Self::PlayerInventory => ScriptCommandCapabilityKind::PlayerInventory,
            Self::Zones => ScriptCommandCapabilityKind::Zones,
            Self::PlayerTeleport => ScriptCommandCapabilityKind::PlayerTeleport,
            Self::PlayerQueries => ScriptCommandCapabilityKind::PlayerQueries,
            Self::WorldTime => ScriptCommandCapabilityKind::WorldTime,
            Self::WorldBlocks => ScriptCommandCapabilityKind::WorldBlocks,
            Self::CustomPayloadChannel { .. } => ScriptCommandCapabilityKind::CustomPayloadChannel,
        }
    }
}

/// Declarative subscription to one Solaris script event name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptEventSubscription {
    event_name: String,
}

impl ScriptEventSubscription {
    pub(super) fn new(event_name: String) -> Self {
        Self { event_name }
    }

    pub fn event_name(&self) -> &str {
        &self.event_name
    }
}

/// Plugin load phase hint for a future script loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ScriptPluginLoadPhase {
    Startup,
    #[default]
    PostWorld,
}

/// Relationship between this plugin and another Solaris plugin id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptPluginDependencyRelation {
    Required,
    Optional,
    LoadBefore,
}

/// Declarative dependency or load-order edge for a future script loader.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptPluginDependency {
    plugin_id: String,
    relation: ScriptPluginDependencyRelation,
}

impl ScriptPluginDependency {
    pub(super) fn new(plugin_id: String, relation: ScriptPluginDependencyRelation) -> Self {
        Self {
            plugin_id,
            relation,
        }
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn relation(&self) -> ScriptPluginDependencyRelation {
        self.relation
    }
}

/// Plugin manifest contract consumed by a future server-side script loader.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScriptPluginManifest {
    plugin_id: String,
    display_name: String,
    version: String,
    requested_api_version: ScriptApiVersion,
    load_phase: ScriptPluginLoadPhase,
    event_subscriptions: Vec<ScriptEventSubscription>,
    dependencies: Vec<ScriptPluginDependency>,
    declared_command_capabilities: Vec<ScriptCommandCapability>,
    player_command_roots: Vec<String>,
    operator_command_roots: Vec<String>,
    declared_permissions: Vec<String>,
    preflight_error: Option<ScriptPluginManifestError>,
}

impl ScriptPluginManifest {
    /// Build a script plugin manifest DTO.
    pub fn new(
        plugin_id: impl AsRef<str>,
        display_name: impl AsRef<str>,
        version: impl AsRef<str>,
        requested_api_version: ScriptApiVersion,
    ) -> Self {
        let mut preflight_error = None;
        let plugin_id = bounded_manifest_owned(
            "plugin id",
            plugin_id.as_ref(),
            MAX_PLUGIN_ID_BYTES,
            &mut preflight_error,
        );
        let display_name = bounded_manifest_owned(
            "display name",
            display_name.as_ref(),
            MAX_PLUGIN_DISPLAY_NAME_BYTES,
            &mut preflight_error,
        );
        let version = bounded_manifest_owned(
            "version",
            version.as_ref(),
            MAX_PLUGIN_VERSION_BYTES,
            &mut preflight_error,
        );
        Self {
            plugin_id,
            display_name,
            version,
            requested_api_version,
            load_phase: ScriptPluginLoadPhase::default(),
            event_subscriptions: Vec::new(),
            dependencies: Vec::new(),
            declared_command_capabilities: Vec::new(),
            player_command_roots: Vec::new(),
            operator_command_roots: Vec::new(),
            declared_permissions: Vec::new(),
            preflight_error,
        }
    }

    /// Declare the preferred load phase for a future loader.
    pub fn with_load_phase(mut self, load_phase: ScriptPluginLoadPhase) -> Self {
        self.load_phase = load_phase;
        self
    }

    /// Declare one capability by the name a manifest writes.
    ///
    /// The vocabulary belongs to the component manifest contract. An unknown
    /// capability name is refused instead of being ignored.
    pub fn declare_capability(self, capability: &str) -> Result<Self, ScriptPluginManifestError> {
        match capability {
            "storage" => Ok(self.declare_plugin_storage()),
            "storage_batches" => Ok(self.declare_storage_batches()),
            "inventory_transfers" => Ok(self.declare_inventory_transfers()),
            "persistent_residents" => Ok(self.declare_persistent_residents()),
            "resident_work" => Ok(self.declare_resident_work()),
            "resident_orders" => Ok(self.declare_resident_orders()),
            "world_sites" => Ok(self.declare_world_sites()),
            "structure_operations" => Ok(self.declare_structure_operations()),
            "inventory_menus" => Ok(self.declare_inventory_menus()),
            "inventory_storage_transactions" => Ok(self.declare_inventory_storage_transactions()),
            "player_inventory" => Ok(self.declare_player_inventory()),
            "zones" => Ok(self.declare_zones()),
            "player_teleport" => Ok(self.declare_player_teleport()),
            "player_queries" => Ok(self.declare_player_queries()),
            "entity_damage" => Ok(self.declare_entity_damage()),
            "world_time" => Ok(self.declare_world_time()),
            "world_blocks" => Ok(self.declare_world_blocks()),
            channel if channel.starts_with("custom_payload:") => {
                Ok(self.declare_custom_payload_channel(&channel["custom_payload:".len()..]))
            }
            _ => Err(ScriptPluginManifestError::InvalidField {
                field: "capability",
            }),
        }
    }

    /// Parse an `api = "MAJOR.MINOR.PATCH"` string.
    pub fn parse_api_version(value: &str) -> Result<ScriptApiVersion, ScriptPluginManifestError> {
        if value.len() > MAX_API_VERSION_BYTES {
            return Err(ScriptPluginManifestError::InvalidField { field: "api" });
        }
        let mut parts = value.split('.');
        let (Some(major), Some(minor), Some(patch)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(ScriptPluginManifestError::InvalidField { field: "api" });
        };
        if parts.next().is_some() {
            return Err(ScriptPluginManifestError::InvalidField { field: "api" });
        }
        let parse = |part: &str| {
            part.parse::<u16>()
                .map_err(|_| ScriptPluginManifestError::InvalidField { field: "api" })
        };
        Ok(ScriptApiVersion::new(
            parse(major)?,
            parse(minor)?,
            parse(patch)?,
        ))
    }

    /// Declare interest in one Solaris-native script event name.
    pub fn subscribe_event(mut self, event_name: impl AsRef<str>) -> Self {
        if self.preflight_error.is_some() {
            return self;
        }
        if self.event_subscriptions.len() >= MAX_MANIFEST_EVENT_SUBSCRIPTIONS {
            self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                field: "event subscriptions",
                max: MAX_MANIFEST_EVENT_SUBSCRIPTIONS,
            });
            return self;
        }
        let event_name = bounded_manifest_owned(
            "event subscription",
            event_name.as_ref(),
            MAX_MANIFEST_FIELD_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            self.event_subscriptions
                .push(ScriptEventSubscription::new(event_name));
        }
        self
    }

    /// Declare a plugin dependency or load-order edge.
    pub fn declare_dependency(
        mut self,
        plugin_id: impl AsRef<str>,
        relation: ScriptPluginDependencyRelation,
    ) -> Self {
        if self.preflight_error.is_some() {
            return self;
        }
        if self.dependencies.len() >= MAX_MANIFEST_DEPENDENCIES {
            self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                field: "dependencies",
                max: MAX_MANIFEST_DEPENDENCIES,
            });
            return self;
        }
        let plugin_id = bounded_manifest_owned(
            "dependency plugin id",
            plugin_id.as_ref(),
            MAX_PLUGIN_ID_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            self.dependencies
                .push(ScriptPluginDependency::new(plugin_id, relation));
        }
        self
    }

    /// Declare one exact entity type this plugin may spawn.
    pub fn declare_spawn_entity_type(mut self, entity_type: impl AsRef<str>) -> Self {
        let entity_type = bounded_manifest_owned(
            "spawn entity type",
            entity_type.as_ref(),
            MAX_SCRIPT_RESOURCE_ID_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            self.push_capability(ScriptCommandCapability::SpawnEntityType { entity_type });
        }
        self
    }

    /// Declare access to the plugin-owned key/value store.
    pub fn declare_plugin_storage(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PluginStorage);
        self
    }

    /// Declare bounded durable operation receipts, storage batches and snapshot scans.
    pub fn declare_storage_batches(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::StorageBatches);
        self
    }

    /// Declare bounded ownership-checked item transfers and durable reservations.
    pub fn declare_inventory_transfers(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::InventoryTransfers);
        self
    }

    /// Declare durable resident handles, lifecycle queries, spawn and release.
    pub fn declare_persistent_residents(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PersistentResidents);
        self
    }

    /// Declare bounded resident work orders and their cancellation.
    pub fn declare_resident_work(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::ResidentWork);
        self
    }

    /// Declare bounded resident squad orders, combat policy and demobilisation.
    pub fn declare_resident_orders(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::ResidentOrders);
        self
    }

    /// Declare settlement site discovery, queries, resident reservation and surveys.
    pub fn declare_world_sites(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::WorldSites);
        self
    }

    /// Declare durable structure preparation, advancement, pause, cancel and status.
    pub fn declare_structure_operations(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::StructureOperations);
        self
    }

    /// Declare access to server-owned inventory menu requests and click events.
    pub fn declare_inventory_menus(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::InventoryMenus);
        self
    }

    /// Declare access to atomic player-inventory and plugin-storage requests.
    pub fn declare_inventory_storage_transactions(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::InventoryStorageTransactions);
        self
    }

    /// Declare access to atomic player main-inventory and hotbar mutations.
    pub fn declare_player_inventory(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PlayerInventory);
        self
    }

    /// Declare access to plugin-owned axis-aligned zones.
    pub fn declare_zones(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::Zones);
        self
    }

    /// Declare access to same-dimension authoritative player teleports.
    pub fn declare_player_teleport(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PlayerTeleport);
        self
    }

    /// Declare access to bounded connected-player snapshots.
    pub fn declare_player_queries(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PlayerQueries);
        self
    }

    /// Declare access to bounded non-player entity damage requests.
    pub fn declare_entity_damage(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::EntityDamage);
        self
    }

    /// Declare access to authoritative world-time mutation requests.
    pub fn declare_world_time(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::WorldTime);
        self
    }

    /// Declare access to authoritative default-state world block mutations.
    pub fn declare_world_blocks(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::WorldBlocks);
        self
    }

    /// Declare exclusive ownership of one namespaced custom-payload channel.
    pub fn declare_custom_payload_channel(mut self, channel: impl AsRef<str>) -> Self {
        let channel = bounded_manifest_owned(
            "custom payload channel",
            channel.as_ref(),
            MAX_SCRIPT_RESOURCE_ID_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            self.push_capability(ScriptCommandCapability::CustomPayloadChannel { channel });
        }
        self
    }

    /// Declare a literal command root that players may invoke for this plugin.
    pub fn declare_player_command_root(mut self, root: impl AsRef<str>) -> Self {
        let root = bounded_manifest_owned(
            "player command root",
            root.as_ref(),
            MAX_PLAYER_COMMAND_ROOT_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            if self.player_command_roots.len() >= MAX_PLAYER_COMMAND_ROOTS {
                self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                    field: "player command roots",
                    max: MAX_PLAYER_COMMAND_ROOTS,
                });
            } else {
                self.player_command_roots.push(root);
            }
        }
        self
    }

    /// Declare a literal player command root that only operators may invoke.
    pub fn declare_operator_command_root(mut self, root: impl AsRef<str>) -> Self {
        let root = bounded_manifest_owned(
            "operator command root",
            root.as_ref(),
            MAX_PLAYER_COMMAND_ROOT_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            if self.operator_command_roots.len() >= MAX_PLAYER_COMMAND_ROOTS {
                self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                    field: "operator command roots",
                    max: MAX_PLAYER_COMMAND_ROOTS,
                });
            } else {
                self.operator_command_roots.push(root);
            }
        }
        self
    }

    /// Declare an opaque plugin permission string for a future loader.
    pub fn declare_permission(mut self, permission: impl AsRef<str>) -> Self {
        let permission = bounded_manifest_owned(
            "permission",
            permission.as_ref(),
            MAX_MANIFEST_FIELD_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            if self.declared_permissions.len() >= MAX_MANIFEST_PERMISSIONS {
                self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                    field: "permissions",
                    max: MAX_MANIFEST_PERMISSIONS,
                });
            } else {
                self.declared_permissions.push(permission);
            }
        }
        self
    }

    fn push_capability(&mut self, capability: ScriptCommandCapability) {
        if self.preflight_error.is_some() {
            return;
        }
        if self.declared_command_capabilities.len() >= MAX_MANIFEST_CAPABILITIES {
            self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                field: "command capabilities",
                max: MAX_MANIFEST_CAPABILITIES,
            });
            return;
        }
        self.declared_command_capabilities.push(capability);
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn requested_api_version(&self) -> ScriptApiVersion {
        self.requested_api_version
    }

    pub fn load_phase(&self) -> ScriptPluginLoadPhase {
        self.load_phase
    }

    pub fn event_subscriptions(&self) -> &[ScriptEventSubscription] {
        &self.event_subscriptions
    }

    pub fn dependencies(&self) -> &[ScriptPluginDependency] {
        &self.dependencies
    }

    pub fn declared_command_capabilities(&self) -> &[ScriptCommandCapability] {
        &self.declared_command_capabilities
    }

    pub fn player_command_roots(&self) -> &[String] {
        &self.player_command_roots
    }

    pub fn operator_command_roots(&self) -> &[String] {
        &self.operator_command_roots
    }

    pub fn declared_permissions(&self) -> &[String] {
        &self.declared_permissions
    }

    /// Validate and normalize this component manifest for trusted host-side use.
    pub fn validate(&self) -> Result<ValidatedScriptPluginManifest, ScriptPluginManifestError> {
        self.validate_for(COMPONENT_PLUGIN_API_VERSION)
    }

    /// Validate this manifest against a specific component contract version.
    ///
    /// The explicit version keeps rejection tests and component-contract upgrades
    /// deterministic; production discovery always supplies
    /// [`COMPONENT_PLUGIN_API_VERSION`].
    pub fn validate_for(
        &self,
        expected_api_version: ScriptApiVersion,
    ) -> Result<ValidatedScriptPluginManifest, ScriptPluginManifestError> {
        if let Some(error) = &self.preflight_error {
            return Err(error.clone());
        }
        validate_manifest_field("plugin id", &self.plugin_id, MAX_PLUGIN_ID_BYTES, false)?;
        validate_manifest_field(
            "display name",
            &self.display_name,
            MAX_PLUGIN_DISPLAY_NAME_BYTES,
            false,
        )?;
        validate_manifest_field("version", &self.version, MAX_PLUGIN_VERSION_BYTES, false)?;
        validate_manifest_count(
            "event subscriptions",
            self.event_subscriptions.len(),
            MAX_MANIFEST_EVENT_SUBSCRIPTIONS,
        )?;
        validate_manifest_count(
            "dependencies",
            self.dependencies.len(),
            MAX_MANIFEST_DEPENDENCIES,
        )?;
        validate_manifest_count(
            "command capabilities",
            self.declared_command_capabilities.len(),
            MAX_MANIFEST_CAPABILITIES,
        )?;
        validate_manifest_count(
            "player command roots",
            self.player_command_roots.len(),
            MAX_PLAYER_COMMAND_ROOTS,
        )?;
        validate_manifest_count(
            "operator command roots",
            self.operator_command_roots.len(),
            MAX_PLAYER_COMMAND_ROOTS,
        )?;
        validate_manifest_count(
            "permissions",
            self.declared_permissions.len(),
            MAX_MANIFEST_PERMISSIONS,
        )?;
        for subscription in &self.event_subscriptions {
            validate_manifest_field(
                "event subscription",
                subscription.event_name(),
                MAX_MANIFEST_FIELD_BYTES,
                false,
            )?;
        }
        for dependency in &self.dependencies {
            validate_manifest_field(
                "dependency plugin id",
                dependency.plugin_id(),
                MAX_PLUGIN_ID_BYTES,
                false,
            )?;
        }
        for capability in &self.declared_command_capabilities {
            match capability {
                ScriptCommandCapability::SpawnEntityType { entity_type } => {
                    validate_manifest_field(
                        "spawn entity type",
                        entity_type,
                        MAX_SCRIPT_RESOURCE_ID_BYTES,
                        false,
                    )?;
                }
                ScriptCommandCapability::CustomPayloadChannel { channel } => {
                    validate_manifest_field(
                        "custom payload channel",
                        channel,
                        MAX_SCRIPT_RESOURCE_ID_BYTES,
                        false,
                    )?;
                }
                _ => {}
            }
        }
        for permission in &self.declared_permissions {
            validate_manifest_field("permission", permission, MAX_MANIFEST_FIELD_BYTES, false)?;
            if !permission.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b':' | b'.' | b'_' | b'-' | b'/')
            }) {
                return Err(ScriptPluginManifestError::InvalidField {
                    field: "permission",
                });
            }
        }
        if self.plugin_id.trim().is_empty() {
            return Err(ScriptPluginManifestError::BlankPluginId);
        }

        if !is_valid_plugin_id(&self.plugin_id) {
            return Err(ScriptPluginManifestError::InvalidPluginId {
                plugin_id: self.plugin_id.clone(),
            });
        }
        if self.display_name.trim().is_empty()
            || self
                .display_name
                .chars()
                .any(|character| character.is_control())
        {
            return Err(ScriptPluginManifestError::InvalidField {
                field: "display name",
            });
        }
        if !is_valid_plugin_version(&self.version) {
            return Err(ScriptPluginManifestError::InvalidField { field: "version" });
        }

        if !supports_plugin_api_version(self.requested_api_version, expected_api_version) {
            return Err(ScriptPluginManifestError::UnsupportedScriptApiVersion {
                requested: self.requested_api_version,
                supported: expected_api_version,
            });
        }

        let mut normalized_event_subscriptions = Vec::with_capacity(self.event_subscriptions.len());
        for subscription in &self.event_subscriptions {
            let event_name = normalize_event_name(subscription.event_name());
            if !is_supported_event_name(&event_name) {
                return Err(ScriptPluginManifestError::InvalidEventName { event_name });
            }
            if normalized_event_subscriptions.iter().any(
                |subscription: &ScriptEventSubscription| subscription.event_name() == event_name,
            ) {
                return Err(ScriptPluginManifestError::DuplicateEventSubscription { event_name });
            }
            normalized_event_subscriptions.push(ScriptEventSubscription::new(event_name));
        }

        let mut normalized_dependencies = Vec::with_capacity(self.dependencies.len());
        for dependency in &self.dependencies {
            let plugin_id = normalize_plugin_id(dependency.plugin_id());
            if plugin_id.is_empty() {
                return Err(ScriptPluginManifestError::BlankDependencyPluginId);
            }
            if !is_valid_plugin_id(&plugin_id) {
                return Err(ScriptPluginManifestError::InvalidDependencyPluginId { plugin_id });
            }
            if plugin_id == self.plugin_id {
                return Err(ScriptPluginManifestError::SelfDependency { plugin_id });
            }
            if normalized_dependencies
                .iter()
                .any(|dependency: &ScriptPluginDependency| dependency.plugin_id() == plugin_id)
            {
                return Err(ScriptPluginManifestError::DuplicateDependency { plugin_id });
            }
            normalized_dependencies.push(ScriptPluginDependency::new(
                plugin_id,
                dependency.relation(),
            ));
        }

        let mut normalized_capabilities =
            Vec::with_capacity(self.declared_command_capabilities.len());
        for capability in &self.declared_command_capabilities {
            match capability {
                ScriptCommandCapability::SpawnEntityType { entity_type } => {
                    let entity_type = validate_script_resource_id(entity_type)?;
                    let spawn_count = normalized_capabilities
                        .iter()
                        .filter(|capability| {
                            matches!(capability, ScriptCommandCapability::SpawnEntityType { .. })
                        })
                        .count();
                    if spawn_count >= MAX_SPAWN_ENTITY_TYPES {
                        return Err(ScriptPluginManifestError::TooManySpawnEntityTypes {
                            max: MAX_SPAWN_ENTITY_TYPES,
                        });
                    }
                    if normalized_capabilities.iter().any(|capability| {
                        matches!(capability, ScriptCommandCapability::SpawnEntityType { entity_type: existing } if existing == &entity_type)
                    }) {
                        return Err(ScriptPluginManifestError::DuplicateSpawnEntityType {
                            entity_type,
                        });
                    }
                    normalized_capabilities
                        .push(ScriptCommandCapability::SpawnEntityType { entity_type });
                }
                ScriptCommandCapability::CustomPayloadChannel { channel } => {
                    let channel = validate_custom_payload_channel(channel)?.to_owned();
                    let capability = ScriptCommandCapability::CustomPayloadChannel { channel };
                    if normalized_capabilities.contains(&capability) {
                        return Err(ScriptPluginManifestError::DuplicateCapability {
                            capability: capability.clone(),
                        });
                    }
                    normalized_capabilities.push(capability);
                }
                ScriptCommandCapability::EntityDamage
                | ScriptCommandCapability::PluginStorage
                | ScriptCommandCapability::StorageBatches
                | ScriptCommandCapability::InventoryTransfers
                | ScriptCommandCapability::PersistentResidents
                | ScriptCommandCapability::ResidentWork
                | ScriptCommandCapability::ResidentOrders
                | ScriptCommandCapability::WorldSites
                | ScriptCommandCapability::StructureOperations
                | ScriptCommandCapability::InventoryMenus
                | ScriptCommandCapability::InventoryStorageTransactions
                | ScriptCommandCapability::PlayerInventory
                | ScriptCommandCapability::Zones
                | ScriptCommandCapability::PlayerTeleport
                | ScriptCommandCapability::PlayerQueries
                | ScriptCommandCapability::WorldTime
                | ScriptCommandCapability::WorldBlocks => {
                    if normalized_capabilities.contains(capability) {
                        return Err(ScriptPluginManifestError::DuplicateCapability {
                            capability: capability.clone(),
                        });
                    }
                    normalized_capabilities.push(capability.clone());
                }
            }
        }

        let mut player_command_roots = Vec::with_capacity(self.player_command_roots.len());
        for root in &self.player_command_roots {
            validate_player_command_root(root)?;
            if BUILT_IN_PLAYER_COMMAND_ROOTS.contains(&root.as_str()) {
                return Err(ScriptPluginManifestError::ReservedPlayerCommandRoot {
                    root: root.clone(),
                });
            }
            if !player_command_roots.contains(root) {
                player_command_roots.push(root.clone());
            }
        }
        let mut operator_command_roots = Vec::with_capacity(self.operator_command_roots.len());
        for root in &self.operator_command_roots {
            validate_player_command_root(root)?;
            if BUILT_IN_PLAYER_COMMAND_ROOTS.contains(&root.as_str()) {
                return Err(ScriptPluginManifestError::ReservedPlayerCommandRoot {
                    root: root.clone(),
                });
            }
            if player_command_roots.contains(root) {
                return Err(ScriptPluginManifestError::ConflictingPlayerCommandRoot {
                    root: root.clone(),
                });
            }
            if !operator_command_roots.contains(root) {
                operator_command_roots.push(root.clone());
            }
        }

        Ok(ValidatedScriptPluginManifest {
            plugin_id: self.plugin_id.clone(),
            display_name: self.display_name.clone(),
            version: self.version.clone(),
            requested_api_version: self.requested_api_version,
            load_phase: self.load_phase,
            event_subscriptions: normalized_event_subscriptions,
            dependencies: normalized_dependencies,
            declared_command_capabilities: normalized_capabilities,
            player_command_roots,
            operator_command_roots,
            declared_permissions: self.declared_permissions.clone(),
        })
    }
}

/// Validated and normalized script plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidatedScriptPluginManifest {
    plugin_id: String,
    display_name: String,
    version: String,
    requested_api_version: ScriptApiVersion,
    load_phase: ScriptPluginLoadPhase,
    event_subscriptions: Vec<ScriptEventSubscription>,
    dependencies: Vec<ScriptPluginDependency>,
    declared_command_capabilities: Vec<ScriptCommandCapability>,
    player_command_roots: Vec<String>,
    operator_command_roots: Vec<String>,
    declared_permissions: Vec<String>,
}

impl ValidatedScriptPluginManifest {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn requested_api_version(&self) -> ScriptApiVersion {
        self.requested_api_version
    }

    pub fn load_phase(&self) -> ScriptPluginLoadPhase {
        self.load_phase
    }

    pub fn event_subscriptions(&self) -> &[ScriptEventSubscription] {
        &self.event_subscriptions
    }

    pub fn dependencies(&self) -> &[ScriptPluginDependency] {
        &self.dependencies
    }

    pub fn declared_command_capabilities(&self) -> &[ScriptCommandCapability] {
        &self.declared_command_capabilities
    }

    pub fn player_command_roots(&self) -> &[String] {
        &self.player_command_roots
    }

    pub fn operator_command_roots(&self) -> &[String] {
        &self.operator_command_roots
    }

    /// Return every custom-payload channel this manifest exclusively owns.
    pub fn custom_payload_channels(&self) -> Vec<&str> {
        self.declared_command_capabilities
            .iter()
            .filter_map(|capability| match capability {
                ScriptCommandCapability::CustomPayloadChannel { channel } => Some(channel.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Trusted host-side conversion from validated manifest declarations to
    /// executable command capabilities.
    ///
    /// Public because the host pre-checks a whole guest batch against exactly the
    /// grants the boundary re-checks when the batch is submitted: one conversion,
    /// so what a package declared is what both sides enforce.
    pub fn to_command_capabilities(&self) -> CommandCapabilities {
        let mut capabilities = CommandCapabilities::none();
        for capability in &self.declared_command_capabilities {
            match capability {
                ScriptCommandCapability::SpawnEntityType { entity_type } => {
                    capabilities = capabilities.allow_spawn_entity_type(entity_type);
                }
                ScriptCommandCapability::EntityDamage => {
                    capabilities = capabilities.allow_entity_damage();
                }
                ScriptCommandCapability::PluginStorage => {
                    capabilities = capabilities.allow_plugin_storage();
                }
                ScriptCommandCapability::StorageBatches => {
                    capabilities = capabilities.allow_storage_batches();
                }
                ScriptCommandCapability::InventoryTransfers => {
                    capabilities = capabilities.allow_inventory_transfers();
                }
                ScriptCommandCapability::PersistentResidents => {
                    capabilities = capabilities.allow_persistent_residents();
                }
                ScriptCommandCapability::ResidentWork => {
                    capabilities = capabilities.allow_resident_work();
                }
                ScriptCommandCapability::ResidentOrders => {
                    capabilities = capabilities.allow_resident_orders();
                }
                ScriptCommandCapability::WorldSites => {
                    capabilities = capabilities.allow_world_sites();
                }
                ScriptCommandCapability::StructureOperations => {
                    capabilities = capabilities.allow_structure_operations();
                }
                ScriptCommandCapability::InventoryMenus => {
                    capabilities = capabilities.allow_inventory_menus();
                }
                ScriptCommandCapability::InventoryStorageTransactions => {
                    capabilities = capabilities.allow_inventory_storage_transactions();
                }
                ScriptCommandCapability::PlayerInventory => {
                    capabilities = capabilities.allow_player_inventory();
                }
                ScriptCommandCapability::Zones => {
                    capabilities = capabilities.allow_zones();
                }
                ScriptCommandCapability::PlayerTeleport => {
                    capabilities = capabilities.allow_player_teleport();
                }
                ScriptCommandCapability::PlayerQueries => {
                    capabilities = capabilities.allow_player_queries();
                }
                ScriptCommandCapability::WorldTime => {
                    capabilities = capabilities.allow_world_time();
                }
                ScriptCommandCapability::WorldBlocks => {
                    capabilities = capabilities.allow_world_blocks();
                }
                ScriptCommandCapability::CustomPayloadChannel { channel } => {
                    capabilities = capabilities.allow_custom_payload_channel(channel);
                }
            }
        }
        capabilities
    }
}

/// Error returned when validating a script plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPluginManifestError {
    FieldTooLong {
        field: &'static str,
        max_bytes: usize,
    },
    EmptyField {
        field: &'static str,
    },
    InvalidField {
        field: &'static str,
    },
    TooManyEntries {
        field: &'static str,
        max: usize,
    },
    BlankPluginId,
    InvalidPluginId {
        plugin_id: String,
    },
    UnsupportedScriptApiVersion {
        requested: ScriptApiVersion,
        supported: ScriptApiVersion,
    },
    InvalidEventName {
        event_name: String,
    },
    DuplicateEventSubscription {
        event_name: String,
    },
    BlankDependencyPluginId,
    InvalidDependencyPluginId {
        plugin_id: String,
    },
    SelfDependency {
        plugin_id: String,
    },
    DuplicateDependency {
        plugin_id: String,
    },
    DuplicateCapability {
        capability: ScriptCommandCapability,
    },
    InvalidSpawnEntityType {
        entity_type: String,
    },
    DuplicateSpawnEntityType {
        entity_type: String,
    },
    TooManySpawnEntityTypes {
        max: usize,
    },
    InvalidCustomPayloadChannel {
        channel: String,
    },
    InvalidPlayerCommandRoot {
        root: String,
    },
    PlayerCommandRootTooLong {
        root: String,
        max_bytes: usize,
    },
    ReservedPlayerCommandRoot {
        root: String,
    },
    ConflictingPlayerCommandRoot {
        root: String,
    },
}

fn validate_manifest_field(
    field: &'static str,
    value: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> Result<(), ScriptPluginManifestError> {
    if !allow_empty && value.is_empty() {
        return Err(ScriptPluginManifestError::EmptyField { field });
    }
    if value.len() > max_bytes {
        return Err(ScriptPluginManifestError::FieldTooLong { field, max_bytes });
    }
    Ok(())
}

fn bounded_manifest_owned(
    field: &'static str,
    value: &str,
    max_bytes: usize,
    error: &mut Option<ScriptPluginManifestError>,
) -> String {
    if error.is_some() {
        return String::new();
    }
    if value.is_empty() {
        *error = Some(ScriptPluginManifestError::EmptyField { field });
        return String::new();
    }
    if value.len() > max_bytes {
        *error = Some(ScriptPluginManifestError::FieldTooLong { field, max_bytes });
        return String::new();
    }
    value.to_owned()
}

fn validate_manifest_count(
    field: &'static str,
    count: usize,
    max: usize,
) -> Result<(), ScriptPluginManifestError> {
    if count > max {
        return Err(ScriptPluginManifestError::TooManyEntries { field, max });
    }
    Ok(())
}

fn is_valid_plugin_version(version: &str) -> bool {
    version.bytes().any(|byte| byte.is_ascii_digit())
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// Allow-list of privileged outbound command capabilities granted by the host.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CommandCapabilities {
    spawn_entity_types: Vec<String>,
    entity_damage: bool,
    plugin_storage: bool,
    storage_batches: bool,
    inventory_transfers: bool,
    persistent_residents: bool,
    resident_work: bool,
    resident_orders: bool,
    world_sites: bool,
    structure_operations: bool,
    inventory_menus: bool,
    inventory_storage_transactions: bool,
    player_inventory: bool,
    zones: bool,
    player_teleport: bool,
    player_queries: bool,
    world_time: bool,
    world_blocks: bool,
    custom_payload_channels: Vec<String>,
}

impl CommandCapabilities {
    /// Return capabilities with no privileged console command roots allowed.
    pub fn none() -> Self {
        Self::default()
    }

    pub(crate) fn allow_spawn_entity_type(mut self, entity_type: impl AsRef<str>) -> Self {
        let entity_type = entity_type.as_ref().to_owned();
        if !self
            .spawn_entity_types
            .iter()
            .any(|allowed| allowed == &entity_type)
        {
            self.spawn_entity_types.push(entity_type);
        }
        self
    }

    pub(crate) fn allow_entity_damage(mut self) -> Self {
        self.entity_damage = true;
        self
    }

    pub(crate) fn allow_plugin_storage(mut self) -> Self {
        self.plugin_storage = true;
        self
    }

    pub(crate) fn allow_storage_batches(mut self) -> Self {
        self.storage_batches = true;
        self
    }

    pub(crate) fn allow_inventory_transfers(mut self) -> Self {
        self.inventory_transfers = true;
        self
    }

    pub(crate) fn allow_persistent_residents(mut self) -> Self {
        self.persistent_residents = true;
        self
    }

    pub(crate) fn allow_resident_work(mut self) -> Self {
        self.resident_work = true;
        self
    }

    pub(crate) fn allow_resident_orders(mut self) -> Self {
        self.resident_orders = true;
        self
    }

    pub(crate) fn allow_world_sites(mut self) -> Self {
        self.world_sites = true;
        self
    }
    pub(crate) fn allow_structure_operations(mut self) -> Self {
        self.structure_operations = true;
        self
    }

    pub(crate) fn allow_inventory_menus(mut self) -> Self {
        self.inventory_menus = true;
        self
    }

    pub(crate) fn allow_inventory_storage_transactions(mut self) -> Self {
        self.inventory_storage_transactions = true;
        self
    }

    pub(crate) fn allow_player_inventory(mut self) -> Self {
        self.player_inventory = true;
        self
    }

    pub(crate) fn allow_zones(mut self) -> Self {
        self.zones = true;
        self
    }

    pub(crate) fn allow_player_teleport(mut self) -> Self {
        self.player_teleport = true;
        self
    }

    pub(crate) fn allow_player_queries(mut self) -> Self {
        self.player_queries = true;
        self
    }

    pub(crate) fn allow_world_time(mut self) -> Self {
        self.world_time = true;
        self
    }

    pub(crate) fn allow_world_blocks(mut self) -> Self {
        self.world_blocks = true;
        self
    }

    pub(crate) fn allow_custom_payload_channel(mut self, channel: impl AsRef<str>) -> Self {
        let channel = channel.as_ref().to_owned();
        if !self
            .custom_payload_channels
            .iter()
            .any(|allowed| allowed == &channel)
        {
            self.custom_payload_channels.push(channel);
        }
        self
    }
    /// Return whether this plugin may exchange one custom-payload channel.
    ///
    /// The query half of the same grant the manifest conversion fills in, for a
    /// host that filters a guest's payload against the grants the boundary
    /// re-checks when the command is submitted.
    #[must_use]
    pub fn allows_custom_payload_channel(&self, channel: &str) -> bool {
        self.custom_payload_channels
            .iter()
            .any(|allowed| allowed == channel)
    }

    pub(super) fn allows(&self, capability: RequiredCommandCapability<'_>) -> bool {
        match capability {
            RequiredCommandCapability::SpawnEntityType { entity_type } => self
                .spawn_entity_types
                .iter()
                .any(|allowed| allowed == entity_type),
            RequiredCommandCapability::EntityDamage => self.entity_damage,
            RequiredCommandCapability::PluginStorage => self.plugin_storage,
            RequiredCommandCapability::StorageBatches => self.storage_batches,
            RequiredCommandCapability::InventoryTransfers => self.inventory_transfers,
            RequiredCommandCapability::PersistentResidents => self.persistent_residents,
            RequiredCommandCapability::ResidentWork => self.resident_work,
            RequiredCommandCapability::ResidentOrders => self.resident_orders,
            RequiredCommandCapability::WorldSites => self.world_sites,
            RequiredCommandCapability::StructureOperations => self.structure_operations,
            RequiredCommandCapability::InventoryMenus => self.inventory_menus,
            RequiredCommandCapability::InventoryStorageTransactions => {
                self.inventory_storage_transactions
            }
            RequiredCommandCapability::PlayerInventory => self.player_inventory,
            RequiredCommandCapability::Zones => self.zones,
            RequiredCommandCapability::PlayerTeleport => self.player_teleport,
            RequiredCommandCapability::PlayerQueries => self.player_queries,
            RequiredCommandCapability::WorldTime => self.world_time,
            RequiredCommandCapability::WorldBlocks => self.world_blocks,
            RequiredCommandCapability::CustomPayloadChannel { channel } => {
                self.allows_custom_payload_channel(channel)
            }
        }
    }
}
fn normalize_event_name(event_name: &str) -> String {
    event_name.trim().to_ascii_lowercase()
}

pub(super) fn is_supported_event_name(event_name: &str) -> bool {
    matches!(
        event_name,
        "server.started"
            | "server.stopping"
            | "player.joined"
            | "player.left"
            | "player.chat"
            | "player.block_broken"
            | "player.block_placed"
            | "player.item_crafted"
            | "player.item_picked_up"
            | "player.entity_killed"
            | "player.entity_interacted"
            | "player.died"
            | "server.tick"
            | "plugin.storage.get_result"
            | "plugin.storage.cas_result"
            | "plugin.storage.delete_result"
            | "inventory.menu.clicked"
            | "inventory.storage_transaction.result"
            | "operation.result"
            | "player.inventory_transaction_result"
            | "player.zone_entered"
            | "player.zone_exited"
            | "zone.command_result"
            | "player.teleport_result"
            | "player.custom_payload"
            | "player.client_brand"
    )
}

fn normalize_plugin_id(plugin_id: &str) -> String {
    plugin_id.trim().to_ascii_lowercase()
}

pub(super) fn is_valid_plugin_id(plugin_id: &str) -> bool {
    let mut chars = plugin_id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }

    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-'))
}

/// Validate one namespaced resource identifier at a manifest boundary.
///
/// A host that accepts resource identifiers from a guest uses this instead of a
/// second copy of the rule, so a resource that a package declares and a resource
/// a runtime call names are accepted and refused identically.
pub fn validate_script_resource_id(value: &str) -> Result<String, ScriptPluginManifestError> {
    if value.len() > MAX_SCRIPT_RESOURCE_ID_BYTES {
        return Err(ScriptPluginManifestError::InvalidSpawnEntityType {
            entity_type: value.to_owned(),
        });
    }
    let Some((namespace, path)) = value.split_once(':') else {
        return Err(ScriptPluginManifestError::InvalidSpawnEntityType {
            entity_type: value.to_owned(),
        });
    };
    if namespace.is_empty()
        || path.is_empty()
        || path.contains(':')
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
        || !path.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'/')
        })
    {
        return Err(ScriptPluginManifestError::InvalidSpawnEntityType {
            entity_type: value.to_owned(),
        });
    }
    Ok(value.to_owned())
}

pub(super) fn validate_custom_payload_channel(
    channel: &str,
) -> Result<&str, ScriptPluginManifestError> {
    // Loader control traffic must use permission-checked typed commands.
    if channel.len() > MAX_SCRIPT_RESOURCE_ID_BYTES || channel.starts_with("solaris:loader/") {
        return Err(ScriptPluginManifestError::InvalidCustomPayloadChannel {
            channel: channel.to_owned(),
        });
    }
    let Some((namespace, path)) = channel.split_once(':') else {
        return Err(ScriptPluginManifestError::InvalidCustomPayloadChannel {
            channel: channel.to_owned(),
        });
    };
    if namespace.is_empty()
        || path.is_empty()
        || path.contains(':')
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
        || !path.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'/')
        })
    {
        return Err(ScriptPluginManifestError::InvalidCustomPayloadChannel {
            channel: channel.to_owned(),
        });
    }
    Ok(channel)
}
fn validate_player_command_root(root: &str) -> Result<(), ScriptPluginManifestError> {
    if root.is_empty()
        || !root.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(ScriptPluginManifestError::InvalidPlayerCommandRoot {
            root: root.to_owned(),
        });
    }
    if root.len() > MAX_PLAYER_COMMAND_ROOT_BYTES {
        return Err(ScriptPluginManifestError::PlayerCommandRootTooLong {
            root: root.to_owned(),
            max_bytes: MAX_PLAYER_COMMAND_ROOT_BYTES,
        });
    }
    Ok(())
}
