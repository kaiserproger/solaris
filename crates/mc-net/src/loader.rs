use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock};

use bytes::BufMut;
use mc_data::{Identifier, ItemStack};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LOADER_PROTOCOL_VERSION: u16 = 3;
pub const MAX_LOADER_MANIFEST_BYTES: usize = 32_767;
pub const MAX_LOADER_VIEW_MESSAGE_BYTES: usize = 64 * 1024;
pub const LOADER_ARTIFACT_CHUNK_BYTES: usize = 30 * 1024;
const LOADER_ARTIFACT_INDEX_PATH: &str = "solaris-client.json";
const MAX_LOADER_ARTIFACT_INDEX_BYTES: u64 = 64 * 1024;
const MAX_LOADER_BLOCK_NAME_BYTES: usize = 128;
const MAX_LOADER_BLOCKS: usize = 8;
const LOADER_VIEW_CHANNEL: &str = "solaris:loader/view";
const LOADER_VIEW_ACTION_CHANNEL: &str = "solaris:loader/view_action";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderPlatform {
    Fabric,
    #[serde(rename = "neoforge")]
    NeoForge,
    Forge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderContentKind {
    Blocks,
    Items,
    Views,
    ViewActions,
    Assets,
    WorldPreviews,
    WorldSelection,
    Sounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderPermission {
    RegisterBlocks,
    RegisterItems,
    PresentViews,
    SendViewActions,
    LoadAssets,
    PresentWorldPreviews,
    SendWorldSelection,
    PlaySounds,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderBundle {
    pub owner: String,
    pub id: String,
    pub version: String,
    pub artifact: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub loaders: Vec<LoaderPlatform>,
    pub content: Vec<LoaderContentKind>,
    pub permissions: Vec<LoaderPermission>,
    pub cache_key: String,
    /// Canonical server-local source; never crosses the wire.
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
    /// Immutable artifact bytes verified at startup; never crosses the wire as manifest data.
    #[serde(skip)]
    pub artifact_bytes: Option<Arc<[u8]>>,
    /// Owner block identity read from the verified artifact; never crosses the wire.
    #[serde(skip)]
    pub block_id: Option<String>,
    /// Owner block display name read from the verified artifact; never crosses the wire.
    #[serde(skip)]
    pub block_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderManifest {
    pub protocol: u16,
    pub bundles: Vec<LoaderBundle>,
}

impl LoaderManifest {
    pub fn from_script_bundles(
        bundles: &[mc_script::LuaClientBundle],
    ) -> Result<Self, LoaderHandshakeError> {
        let bundles = bundles
            .iter()
            .map(|bundle| {
                let block = read_declared_block(bundle)?;
                Ok(LoaderBundle {
                    owner: bundle.owner_plugin_id().to_owned(),
                    id: bundle.id().to_owned(),
                    version: bundle.version().to_owned(),
                    artifact: bundle.artifact().to_owned(),
                    sha256: bundle.sha256().to_owned(),
                    size_bytes: bundle.size_bytes(),
                    loaders: bundle
                        .loaders()
                        .iter()
                        .map(|loader| match loader {
                            mc_script::LuaClientLoader::Fabric => LoaderPlatform::Fabric,
                            mc_script::LuaClientLoader::NeoForge => LoaderPlatform::NeoForge,
                            mc_script::LuaClientLoader::Forge => LoaderPlatform::Forge,
                            _ => unreachable!("validated client loader"),
                        })
                        .collect(),
                    content: bundle
                        .content()
                        .iter()
                        .map(|content| match content {
                            mc_script::LuaClientContentKind::Blocks => LoaderContentKind::Blocks,
                            mc_script::LuaClientContentKind::Items => LoaderContentKind::Items,
                            mc_script::LuaClientContentKind::Views => LoaderContentKind::Views,
                            mc_script::LuaClientContentKind::ViewActions => {
                                LoaderContentKind::ViewActions
                            }
                            mc_script::LuaClientContentKind::Assets => LoaderContentKind::Assets,
                            mc_script::LuaClientContentKind::WorldPreviews => {
                                LoaderContentKind::WorldPreviews
                            }
                            mc_script::LuaClientContentKind::WorldSelection => {
                                LoaderContentKind::WorldSelection
                            }
                            mc_script::LuaClientContentKind::Sounds => LoaderContentKind::Sounds,
                            _ => unreachable!("validated client content kind"),
                        })
                        .collect(),
                    permissions: bundle
                        .permissions()
                        .iter()
                        .map(|permission| match permission {
                            mc_script::LuaClientPermission::RegisterBlocks => {
                                LoaderPermission::RegisterBlocks
                            }
                            mc_script::LuaClientPermission::RegisterItems => {
                                LoaderPermission::RegisterItems
                            }
                            mc_script::LuaClientPermission::PresentViews => {
                                LoaderPermission::PresentViews
                            }
                            mc_script::LuaClientPermission::SendViewActions => {
                                LoaderPermission::SendViewActions
                            }
                            mc_script::LuaClientPermission::LoadAssets => {
                                LoaderPermission::LoadAssets
                            }
                            mc_script::LuaClientPermission::PresentWorldPreviews => {
                                LoaderPermission::PresentWorldPreviews
                            }
                            mc_script::LuaClientPermission::SendWorldSelection => {
                                LoaderPermission::SendWorldSelection
                            }
                            mc_script::LuaClientPermission::PlaySounds => {
                                LoaderPermission::PlaySounds
                            }
                            _ => unreachable!("validated client permission"),
                        })
                        .collect(),
                    cache_key: bundle.cache_key(),
                    source_path: Some(bundle.artifact_path().to_path_buf()),
                    artifact_bytes: Some(bundle.artifact_bytes_arc()),
                    block_id: block.as_ref().map(|block| block.id.clone()),
                    block_name: block.map(|block| block.name),
                })
            })
            .collect::<Result<Vec<_>, LoaderHandshakeError>>()?;
        let block_ids = bundles
            .iter()
            .filter_map(|bundle| bundle.block_id.as_deref())
            .collect::<BTreeSet<_>>();
        let block_count = bundles
            .iter()
            .filter(|bundle| bundle.block_id.is_some())
            .count();
        if block_count > MAX_LOADER_BLOCKS {
            return Err(LoaderHandshakeError::ArtifactIndex(format!(
                "Loader artifacts declare more than {MAX_LOADER_BLOCKS} blocks"
            )));
        }
        if block_ids.len() != block_count {
            return Err(LoaderHandshakeError::ArtifactIndex(
                "Loader artifacts declare duplicate block identities".to_owned(),
            ));
        }
        Ok(Self {
            protocol: LOADER_PROTOCOL_VERSION,
            bundles,
        })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bundles.is_empty()
    }

    pub fn encode(&self) -> Result<Vec<u8>, LoaderHandshakeError> {
        let payload = serde_json::to_vec(self)
            .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
        if payload.len() > MAX_LOADER_MANIFEST_BYTES {
            return Err(LoaderHandshakeError::ManifestTooLarge {
                len: payload.len(),
                max: MAX_LOADER_MANIFEST_BYTES,
            });
        }
        Ok(payload)
    }

    pub fn validate_ack(&self, ack: &LoaderClientAck) -> Result<(), LoaderHandshakeError> {
        if ack.protocol != self.protocol {
            return Err(LoaderHandshakeError::Protocol {
                expected: self.protocol,
                actual: ack.protocol,
            });
        }
        if ack.loader_version.is_empty() || ack.loader_version.len() > 64 {
            return Err(LoaderHandshakeError::InvalidLoaderVersion);
        }
        let accepted = ack
            .accepted_permissions
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let cached = ack
            .cached_bundles
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for bundle in &self.bundles {
            if !bundle.loaders.contains(&ack.platform) {
                return Err(LoaderHandshakeError::UnsupportedPlatform {
                    owner: bundle.owner.clone(),
                    bundle: bundle.id.clone(),
                    platform: ack.platform,
                });
            }
            if let Some(permission) = bundle
                .permissions
                .iter()
                .find(|permission| !accepted.contains(permission))
            {
                return Err(LoaderHandshakeError::PermissionDenied {
                    owner: bundle.owner.clone(),
                    bundle: bundle.id.clone(),
                    permission: *permission,
                });
            }
            if !cached.contains(bundle.cache_key.as_str()) {
                return Err(LoaderHandshakeError::BundleUnavailable {
                    owner: bundle.owner.clone(),
                    bundle: bundle.id.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn bind_ack(&self, ack: &LoaderClientAck) -> Result<LoaderSession, LoaderHandshakeError> {
        self.validate_ack(ack)?;
        let block_ids = self
            .bundles
            .iter()
            .filter_map(|bundle| bundle.block_id.as_deref())
            .collect::<BTreeSet<_>>();
        let mut block_states = BTreeMap::new();
        let mut block_names = BTreeMap::new();
        if block_ids.is_empty() && !ack.carrier_block_state_ids.is_empty() {
            return Err(LoaderHandshakeError::UnexpectedBlockCarrierState);
        }
        if !block_ids.is_empty() && ack.carrier_block_state_ids.is_empty() {
            return Err(LoaderHandshakeError::MissingBlockCarrierState);
        }
        let acknowledged_ids = ack
            .carrier_block_state_ids
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if acknowledged_ids != block_ids {
            return Err(LoaderHandshakeError::BlockCarrierSetMismatch);
        }
        let mut carrier_states = BTreeSet::new();
        for id in block_ids {
            let state_id = ack.carrier_block_state_ids[id];
            if state_id > i32::MAX as u32 || !carrier_states.insert(state_id) {
                return Err(LoaderHandshakeError::InvalidBlockCarrierState { state_id });
            }
            block_states.insert(id.to_owned(), state_id);
            if let Some(name) = self
                .bundles
                .iter()
                .find(|bundle| bundle.block_id.as_deref() == Some(id))
                .and_then(|bundle| bundle.block_name.as_deref())
            {
                block_names.insert(id.to_owned(), name.to_owned());
            }
        }
        Ok(LoaderSession {
            platform: ack.platform,
            loader_version: ack.loader_version.clone(),
            block_states,
            block_names,
        })
    }

    /// Add Loader-owned blocks to Solaris' canonical server registry.
    pub fn append_world_block_report(
        &self,
        report: &mut Vec<mc_data::blocks::BlockReport>,
    ) -> Result<Vec<u32>, LoaderHandshakeError> {
        let block_ids = self
            .bundles
            .iter()
            .filter_map(|bundle| bundle.block_id.as_deref())
            .collect::<BTreeSet<_>>();
        let mut next_state_id = report
            .iter()
            .try_fold(0_u32, |count, block| {
                u32::try_from(block.states.len())
                    .ok()
                    .and_then(|states| count.checked_add(states))
            })
            .ok_or_else(|| {
                LoaderHandshakeError::ArtifactIndex(
                    "server block-state registry exceeds u32".to_owned(),
                )
            })?;
        let mut state_ids = Vec::with_capacity(block_ids.len());
        for block_id in block_ids {
            let id = Identifier::parse(block_id.to_owned()).map_err(|error| {
                LoaderHandshakeError::ArtifactIndex(format!(
                    "invalid Loader block identity {block_id}: {error}"
                ))
            })?;
            report.push(mc_data::blocks::BlockReport {
                id,
                properties: BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: next_state_id,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            });
            state_ids.push(next_state_id);
            next_state_id = next_state_id.checked_add(1).ok_or_else(|| {
                LoaderHandshakeError::ArtifactIndex(
                    "server block-state registry exceeds u32".to_owned(),
                )
            })?;
        }
        Ok(state_ids)
    }

    #[must_use]
    pub(crate) fn world_block_state(
        &self,
        plugin_id: &str,
        block_id: &str,
        blocks: &mc_world::BlockRegistry,
    ) -> Option<mc_world::BlockStateId> {
        self.bundles.iter().find(|bundle| {
            bundle.owner == plugin_id
                && bundle.block_id.as_deref() == Some(block_id)
                && bundle
                    .permissions
                    .contains(&LoaderPermission::RegisterBlocks)
        })?;
        let id = Identifier::parse(block_id.to_owned()).ok()?;
        blocks.block(&id).map(|block| block.default)
    }

    #[must_use]
    pub(crate) fn world_block_item(
        &self,
        plugin_id: &str,
        block_id: &str,
        count: u8,
        items: &mc_data::items::ItemRegistry,
    ) -> Option<ItemStack> {
        let bundle = self.bundles.iter().find(|bundle| {
            bundle.owner == plugin_id
                && bundle.block_id.as_deref() == Some(block_id)
                && bundle
                    .permissions
                    .contains(&LoaderPermission::RegisterBlocks)
        })?;
        let name = bundle.block_name.as_deref()?;
        let carrier_index = self.block_carrier_index(block_id)?;
        let paper = Identifier::parse("minecraft:paper").ok()?;
        let paper_id = items.id_of(&paper)?;
        Some(
            ItemStack::new(paper_id, i32::from(count))
                .with_custom_name(name)
                .with_item_model(loader_block_item_model(carrier_index)),
        )
    }

    fn block_carrier_index(&self, block_id: &str) -> Option<usize> {
        self.bundles
            .iter()
            .filter_map(|bundle| bundle.block_id.as_deref())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .position(|candidate| candidate == block_id)
    }

    pub fn requested_artifact(
        &self,
        request: &LoaderArtifactRequest,
    ) -> Result<(&LoaderBundle, &[u8]), LoaderHandshakeError> {
        if request.protocol != self.protocol {
            return Err(LoaderHandshakeError::Protocol {
                expected: self.protocol,
                actual: request.protocol,
            });
        }
        let bundle = self
            .bundles
            .iter()
            .find(|bundle| bundle.cache_key == request.cache_key)
            .ok_or_else(|| LoaderHandshakeError::UnknownBundleRequest {
                cache_key: request.cache_key.clone(),
            })?;
        let bytes = bundle.artifact_bytes.as_deref().ok_or_else(|| {
            LoaderHandshakeError::ArtifactUnavailable {
                cache_key: request.cache_key.clone(),
            }
        })?;
        Ok((bundle, bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderClientAck {
    pub protocol: u16,
    pub platform: LoaderPlatform,
    pub loader_version: String,
    pub accepted_permissions: Vec<LoaderPermission>,
    pub cached_bundles: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub carrier_block_state_ids: BTreeMap<String, u32>,
}

impl LoaderClientAck {
    pub fn decode(payload: &[u8]) -> Result<Self, LoaderHandshakeError> {
        serde_json::from_slice(payload)
            .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderSession {
    platform: LoaderPlatform,
    loader_version: String,
    block_states: BTreeMap<String, u32>,
    block_names: BTreeMap<String, String>,
}

/// One authoritative Loader block state in Solaris' server-owned state space.
///
/// Its id comes from Solaris' canonical registry. It is never a client runtime
/// id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoaderWorldBlockState {
    owner_block_id: String,
    item_name: String,
    item_model: Identifier,
    canonical_state_id: mc_world::BlockStateId,
}

/// Connection-scoped projection from the server-owned Loader state to the
/// carrier state negotiated by this exact client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoaderBlockProjection {
    world_states: BTreeMap<mc_world::BlockStateId, LoaderWorldBlockState>,
    carrier_states: BTreeMap<mc_world::BlockStateId, mc_world::BlockStateId>,
}

impl LoaderBlockProjection {
    #[must_use]
    pub(crate) fn canonical_state_for_item_model(
        &self,
        item_model: &Identifier,
    ) -> Option<mc_world::BlockStateId> {
        self.world_states
            .values()
            .find(|state| &state.item_model == item_model)
            .map(|state| state.canonical_state_id)
    }

    pub(crate) fn item_stack_for_state(
        &self,
        items: &mc_data::items::ItemRegistry,
        state: mc_world::BlockStateId,
        count: i32,
    ) -> Option<ItemStack> {
        let world_state = self.world_states.get(&state)?;
        let paper = Identifier::parse("minecraft:paper").expect("static paper item id");
        let paper_id = items.id_of(&paper)?;
        Some(
            ItemStack::new(paper_id, count)
                .with_custom_name(&world_state.item_name)
                .with_item_model(world_state.item_model.clone()),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test(canonical_state_id: u32, carrier_state_id: u32) -> Self {
        let canonical_state_id = mc_world::BlockStateId(canonical_state_id);
        Self {
            world_states: BTreeMap::from([(
                canonical_state_id,
                LoaderWorldBlockState {
                    owner_block_id: "example:test_block".to_owned(),
                    item_name: "Test Block".to_owned(),
                    item_model: loader_block_item_model(0),
                    canonical_state_id,
                },
            )]),
            carrier_states: BTreeMap::from([(
                canonical_state_id,
                mc_world::BlockStateId(carrier_state_id),
            )]),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn world_state(
        &self,
        canonical_state_id: mc_world::BlockStateId,
    ) -> Option<&LoaderWorldBlockState> {
        self.world_states.get(&canonical_state_id)
    }

    #[must_use]
    pub(crate) fn project(&self, state: mc_world::BlockStateId) -> mc_world::BlockStateId {
        self.carrier_states.get(&state).copied().unwrap_or(state)
    }
}

#[must_use]
pub(crate) fn loader_block_item_model(index: usize) -> Identifier {
    let path = if index == 0 {
        "loader_block".to_owned()
    } else {
        format!("loader_block_{index}")
    };
    Identifier::parse(format!("solaris_loader:{path}")).expect("valid Loader block item model")
}

#[must_use]
pub(crate) fn is_loader_block_item_model(model: &Identifier) -> bool {
    (0..MAX_LOADER_BLOCKS).any(|index| loader_block_item_model(index) == *model)
}

impl LoaderWorldBlockState {
    #[cfg(test)]
    #[must_use]
    pub(crate) fn owner_block_id(&self) -> &str {
        &self.owner_block_id
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) const fn canonical_state_id(&self) -> mc_world::BlockStateId {
        self.canonical_state_id
    }
}

impl LoaderSession {
    #[must_use]
    pub const fn platform(&self) -> LoaderPlatform {
        self.platform
    }

    #[must_use]
    pub fn loader_version(&self) -> &str {
        &self.loader_version
    }

    #[must_use]
    pub fn block_state_id(&self, owner_block_id: &str) -> Option<u32> {
        self.block_states.get(owner_block_id).copied()
    }

    #[must_use]
    pub(crate) fn block_projection(
        &self,
        blocks: &mc_world::BlockRegistry,
    ) -> Option<LoaderBlockProjection> {
        let mut world_states = BTreeMap::new();
        let mut carrier_states = BTreeMap::new();
        for (index, (owner_block_id, carrier_state_id)) in self.block_states.iter().enumerate() {
            let item_name = self.block_names.get(owner_block_id)?;
            let owner_id = Identifier::parse(owner_block_id.clone()).ok()?;
            let canonical_state_id = blocks.block(&owner_id)?.default;
            world_states.insert(
                canonical_state_id,
                LoaderWorldBlockState {
                    owner_block_id: owner_block_id.clone(),
                    item_name: item_name.clone(),
                    item_model: loader_block_item_model(index),
                    canonical_state_id,
                },
            );
            carrier_states.insert(
                canonical_state_id,
                mc_world::BlockStateId(*carrier_state_id),
            );
        }
        (!world_states.is_empty()).then_some(LoaderBlockProjection {
            world_states,
            carrier_states,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoaderArtifactIndex {
    schema: u16,
    #[serde(default)]
    screens: Vec<serde_json::Value>,
    #[serde(default)]
    world_previews: Vec<serde_json::Value>,
    #[serde(default)]
    blocks: Vec<LoaderArtifactBlock>,
    #[serde(default)]
    items: Vec<serde_json::Value>,
    #[serde(default)]
    assets: Vec<serde_json::Value>,
    #[serde(default)]
    sounds: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoaderArtifactBlock {
    id: String,
    model: String,
    name: String,
}

#[derive(Debug)]
struct VerifiedLoaderBlock {
    id: String,
    name: String,
}

fn read_declared_block(
    bundle: &mc_script::LuaClientBundle,
) -> Result<Option<VerifiedLoaderBlock>, LoaderHandshakeError> {
    if !bundle
        .content()
        .contains(&mc_script::LuaClientContentKind::Blocks)
    {
        return Ok(None);
    }
    read_block_from_artifact_bytes(
        bundle.artifact_bytes(),
        bundle.artifact_path(),
        bundle.owner_plugin_id(),
        bundle.size_bytes(),
        bundle.sha256(),
    )
    .map(Some)
}

fn read_block_from_artifact_bytes(
    bytes: &[u8],
    artifact_path: &Path,
    owner: &str,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<VerifiedLoaderBlock, LoaderHandshakeError> {
    if bytes.len() as u64 != expected_size
        || format!("{:x}", Sha256::digest(bytes)) != expected_sha256
    {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "{} changed after plugin artifact verification",
            artifact_path.display()
        )));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        LoaderHandshakeError::ArtifactIndex(format!(
            "opening {} as ZIP: {error}",
            artifact_path.display()
        ))
    })?;
    let mut entry = archive.by_index(0).map_err(|error| {
        LoaderHandshakeError::ArtifactIndex(format!(
            "reading the first entry from {}: {error}",
            artifact_path.display()
        ))
    })?;
    if entry.name() != LOADER_ARTIFACT_INDEX_PATH || entry.is_dir() {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "{} must begin with {LOADER_ARTIFACT_INDEX_PATH}",
            artifact_path.display()
        )));
    }
    if entry.size() > MAX_LOADER_ARTIFACT_INDEX_BYTES {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "{} exceeds {MAX_LOADER_ARTIFACT_INDEX_BYTES} bytes",
            LOADER_ARTIFACT_INDEX_PATH
        )));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .by_ref()
        .take(MAX_LOADER_ARTIFACT_INDEX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| LoaderHandshakeError::ArtifactIndex(error.to_string()))?;
    if bytes.len() as u64 > MAX_LOADER_ARTIFACT_INDEX_BYTES {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "{} exceeds {MAX_LOADER_ARTIFACT_INDEX_BYTES} bytes",
            LOADER_ARTIFACT_INDEX_PATH
        )));
    }
    let index: LoaderArtifactIndex = serde_json::from_slice(&bytes)
        .map_err(|error| LoaderHandshakeError::ArtifactIndex(error.to_string()))?;
    let LoaderArtifactIndex {
        schema,
        screens,
        world_previews,
        blocks,
        items,
        assets,
        sounds,
    } = index;
    let _ = (screens, world_previews, items, assets, sounds);
    if schema != 2 {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "unsupported Loader artifact index schema {schema}"
        )));
    }
    let [block] = blocks.as_slice() else {
        return Err(LoaderHandshakeError::ArtifactIndex(
            "block bundle must declare exactly one block identity".to_owned(),
        ));
    };
    require_owned_block_id(&block.id, owner)?;
    if block.model.is_empty()
        || block.name.is_empty()
        || block.name.len() > MAX_LOADER_BLOCK_NAME_BYTES
    {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "Loader block model must be non-empty and name must contain 1..={MAX_LOADER_BLOCK_NAME_BYTES} bytes"
        )));
    }
    Ok(VerifiedLoaderBlock {
        id: block.id.clone(),
        name: block.name.clone(),
    })
}

fn require_owned_block_id(id: &str, owner: &str) -> Result<(), LoaderHandshakeError> {
    let prefix = format!("{owner}:");
    if id.len() > 128 || !id.starts_with(&prefix) || id.len() == prefix.len() {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "Loader block identity {id:?} must be owned by {owner}"
        )));
    }
    for (index, byte) in id.bytes().enumerate() {
        let separator = index == owner.len() && byte == b':';
        let allowed = byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'_' | b'.' | b'-')
            || (index > owner.len() && byte == b'/');
        if !separator && !allowed {
            return Err(LoaderHandshakeError::ArtifactIndex(format!(
                "Loader block identity {id:?} contains invalid characters"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderArtifactRequest {
    pub protocol: u16,
    pub cache_key: String,
}

/// One wire-3 view action admitted from a Loader client.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoaderViewActionRequest {
    pub(crate) view_instance_id: String,
    pub(crate) view_revision: u64,
    pub(crate) action_id: String,
    pub(crate) action_sequence: u64,
    pub(crate) fields: Vec<mc_script::ScriptClientViewField>,
    pub(crate) selection_token: Option<String>,
}

/// One decoded wire-3 client request from the Loader action channel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LoaderViewClientRequest {
    /// Explicit key-driven open request: `view_request { request_kind }`.
    ViewRequest {
        request_kind: mc_script::ScriptClientViewRequestKind,
    },
    Action(LoaderViewActionRequest),
    CancelSelection {
        selection_context_id: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireViewField {
    id: String,
    #[serde(default)]
    number: Option<f64>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    selected: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireViewAction {
    protocol: u16,
    #[allow(dead_code)]
    message: String,
    view_instance_id: String,
    view_revision: u64,
    action_id: String,
    action_sequence: u64,
    #[serde(default)]
    fields: Vec<WireViewField>,
    #[serde(default)]
    selection_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireViewRequest {
    protocol: u16,
    #[allow(dead_code)]
    message: String,
    request_kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCancelSelection {
    protocol: u16,
    #[allow(dead_code)]
    message: String,
    selection_context_id: String,
}

fn decode_wire_view_field(
    field: WireViewField,
) -> Result<mc_script::ScriptClientViewField, LoaderHandshakeError> {
    let typed =
        field.number.is_some() as u8 + field.text.is_some() as u8 + field.selected.is_some() as u8;
    if typed != 1 {
        return Err(LoaderHandshakeError::Malformed(format!(
            "view field {:?} must carry exactly one typed value",
            field.id
        )));
    }
    let parsed = if let Some(number) = field.number {
        mc_script::ScriptClientViewField::try_number(&field.id, number)
    } else if let Some(text) = field.text {
        mc_script::ScriptClientViewField::try_text(&field.id, text)
    } else {
        mc_script::ScriptClientViewField::try_selected(
            &field.id,
            field.selected.as_deref().unwrap_or_default(),
        )
    };
    parsed.map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))
}

/// Decode one closed-schema wire-3 client request; unknown fields, unknown
/// messages and a wrong protocol all fail closed.
pub(crate) fn decode_loader_view_client_request(
    payload: &[u8],
) -> Result<LoaderViewClientRequest, LoaderHandshakeError> {
    if payload.is_empty() || payload.len() > MAX_LOADER_VIEW_MESSAGE_BYTES {
        return Err(LoaderHandshakeError::Malformed(format!(
            "view request size is outside 1..={MAX_LOADER_VIEW_MESSAGE_BYTES}"
        )));
    }
    let document: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
    let message = document
        .get("message")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| LoaderHandshakeError::Malformed("view request has no message".to_owned()))?
        .to_owned();
    match message.as_str() {
        "view_action" => {
            let action: WireViewAction = serde_json::from_value(document)
                .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
            require_view_protocol(action.protocol)?;
            if action.fields.len() > mc_script::MAX_CLIENT_VIEW_FIELDS {
                return Err(LoaderHandshakeError::Malformed(
                    "view action fields exceed the limit".to_owned(),
                ));
            }
            let fields = action
                .fields
                .into_iter()
                .map(decode_wire_view_field)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LoaderViewClientRequest::Action(LoaderViewActionRequest {
                view_instance_id: action.view_instance_id,
                view_revision: action.view_revision,
                action_id: action.action_id,
                action_sequence: action.action_sequence,
                fields,
                selection_token: action.selection_token,
            }))
        }
        "cancel_selection" => {
            let cancel: WireCancelSelection = serde_json::from_value(document)
                .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
            require_view_protocol(cancel.protocol)?;
            Ok(LoaderViewClientRequest::CancelSelection {
                selection_context_id: cancel.selection_context_id,
            })
        }
        "view_request" => {
            let request: WireViewRequest = serde_json::from_value(document)
                .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
            require_view_protocol(request.protocol)?;
            let request_kind =
                mc_script::ScriptClientViewRequestKind::from_contract_name(&request.request_kind)
                    .ok_or_else(|| {
                    LoaderHandshakeError::Malformed(format!(
                        "unknown view request kind {:?}",
                        request.request_kind
                    ))
                })?;
            Ok(LoaderViewClientRequest::ViewRequest { request_kind })
        }
        other => Err(LoaderHandshakeError::Malformed(format!(
            "unknown Loader view message {other:?}"
        ))),
    }
}

fn require_view_protocol(protocol: u16) -> Result<(), LoaderHandshakeError> {
    if protocol != LOADER_PROTOCOL_VERSION {
        return Err(LoaderHandshakeError::Protocol {
            expected: LOADER_PROTOCOL_VERSION,
            actual: protocol,
        });
    }
    Ok(())
}

/// One wire-3 server-to-client view message.
#[derive(Debug, Clone, Copy)]
pub(crate) enum LoaderViewWireMessage<'a> {
    Open {
        view_instance_id: &'a str,
        revision: u64,
        view_id: &'a str,
        title: &'a str,
        model: &'a mc_script::ScriptClientViewModel,
    },
    Present {
        view_instance_id: &'a str,
        revision: u64,
        model: &'a mc_script::ScriptClientViewModel,
    },
    Close {
        view_instance_id: &'a str,
    },
}

/// Encode one wire-3 message, failing closed when it exceeds the packet bound.
pub(crate) fn encode_loader_view_message(
    message: &LoaderViewWireMessage<'_>,
) -> Result<Vec<u8>, LoaderHandshakeError> {
    let document = match message {
        LoaderViewWireMessage::Open {
            view_instance_id,
            revision,
            view_id,
            title,
            model,
        } => serde_json::json!({
            "protocol": LOADER_PROTOCOL_VERSION,
            "message": "open_view",
            "view_instance_id": view_instance_id,
            "revision": revision,
            "view_id": view_id,
            "title": title,
            "model": model,
        }),
        LoaderViewWireMessage::Present {
            view_instance_id,
            revision,
            model,
        } => serde_json::json!({
            "protocol": LOADER_PROTOCOL_VERSION,
            "message": "present_view",
            "view_instance_id": view_instance_id,
            "revision": revision,
            "model": model,
        }),
        LoaderViewWireMessage::Close { view_instance_id } => serde_json::json!({
            "protocol": LOADER_PROTOCOL_VERSION,
            "message": "close_view",
            "view_instance_id": view_instance_id,
        }),
    };
    let payload = serde_json::to_vec(&document)
        .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
    if payload.len() > MAX_LOADER_VIEW_MESSAGE_BYTES {
        return Err(LoaderHandshakeError::Malformed(format!(
            "view message is {} bytes, maximum is {MAX_LOADER_VIEW_MESSAGE_BYTES}",
            payload.len()
        )));
    }
    Ok(payload)
}

impl LoaderArtifactRequest {
    pub fn decode(payload: &[u8]) -> Result<Self, LoaderHandshakeError> {
        serde_json::from_slice(payload)
            .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))
    }
}

pub fn encode_artifact_chunk(
    cache_key: &str,
    offset: u64,
    last: bool,
    bytes: &[u8],
) -> Result<Vec<u8>, LoaderHandshakeError> {
    let cache_key = cache_key.as_bytes();
    if cache_key.is_empty() || cache_key.len() > u16::MAX as usize {
        return Err(LoaderHandshakeError::Malformed(
            "artifact cache key length is outside 1..=65535".to_owned(),
        ));
    }
    if bytes.is_empty() || bytes.len() > LOADER_ARTIFACT_CHUNK_BYTES {
        return Err(LoaderHandshakeError::Malformed(format!(
            "artifact chunk length is outside 1..={LOADER_ARTIFACT_CHUNK_BYTES}"
        )));
    }
    let mut payload = Vec::with_capacity(2 + 2 + cache_key.len() + 8 + 1 + bytes.len());
    payload.put_u16(LOADER_PROTOCOL_VERSION);
    payload
        .put_u16(u16::try_from(cache_key.len()).expect("cache key length was checked against u16"));
    payload.put_slice(cache_key);
    payload.put_u64(offset);
    payload.put_u8(u8::from(last));
    payload.put_slice(bytes);
    Ok(payload)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoaderHandshakeError {
    #[error("loader payload is malformed: {0}")]
    Malformed(String),
    #[error("loader manifest is {len} bytes, maximum is {max}")]
    ManifestTooLarge { len: usize, max: usize },
    #[error("loader protocol mismatch: expected {expected}, got {actual}")]
    Protocol { expected: u16, actual: u16 },
    #[error("loader version must contain 1..=64 bytes")]
    InvalidLoaderVersion,
    #[error("bundle {owner}:{bundle} does not support platform {platform:?}")]
    UnsupportedPlatform {
        owner: String,
        bundle: String,
        platform: LoaderPlatform,
    },
    #[error("bundle {owner}:{bundle} permission {permission:?} was not accepted")]
    PermissionDenied {
        owner: String,
        bundle: String,
        permission: LoaderPermission,
    },
    #[error("bundle {owner}:{bundle} is not present in the client cache")]
    BundleUnavailable { owner: String, bundle: String },
    #[error("client requested unknown loader cache identity {cache_key}")]
    UnknownBundleRequest { cache_key: String },
    #[error("server artifact is unavailable for loader cache identity {cache_key}")]
    ArtifactUnavailable { cache_key: String },
    #[error("loader artifact index is invalid: {0}")]
    ArtifactIndex(String),
    #[error("client omitted the Loader block carrier state")]
    MissingBlockCarrierState,
    #[error("client reported a Loader block carrier state without an acknowledged block")]
    UnexpectedBlockCarrierState,
    #[error("client reported invalid Loader block carrier state {state_id}")]
    InvalidBlockCarrierState { state_id: u32 },
    #[error("client Loader block carrier identities do not match the manifest")]
    BlockCarrierSetMismatch,
}

#[must_use]
pub fn loader_manifest_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        Identifier::parse("solaris:loader/manifest").expect("static loader manifest channel")
    })
}

pub(crate) fn loader_view_channel() -> &'static Identifier {
    static CHANNEL: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::parse(LOADER_VIEW_CHANNEL).expect("static Solaris Loader view channel is valid")
    });
    &CHANNEL
}

pub(crate) fn loader_view_action_channel() -> &'static Identifier {
    static CHANNEL: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::parse(LOADER_VIEW_ACTION_CHANNEL)
            .expect("static Solaris Loader view action channel is valid")
    });
    &CHANNEL
}

impl LoaderManifest {
    /// True when `plugin_id` owns a bundle declaring both the content kind and
    /// its permission pair. Permission revocation re-runs this check per action.
    pub(crate) fn plugin_grants(
        &self,
        plugin_id: &str,
        content: LoaderContentKind,
        permission: LoaderPermission,
    ) -> bool {
        self.bundles.iter().any(|bundle| {
            bundle.owner == plugin_id
                && bundle.content.contains(&content)
                && bundle.permissions.contains(&permission)
        })
    }
}

#[must_use]
pub fn loader_ack_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL
        .get_or_init(|| Identifier::parse("solaris:loader/ack").expect("static loader ack channel"))
}

#[must_use]
pub fn loader_request_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        Identifier::parse("solaris:loader/request").expect("static loader request channel")
    })
}

#[must_use]
pub fn loader_artifact_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        Identifier::parse("solaris:loader/artifact").expect("static loader artifact channel")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn manifest() -> LoaderManifest {
        LoaderManifest {
            protocol: LOADER_PROTOCOL_VERSION,
            bundles: vec![LoaderBundle {
                owner: "example".to_owned(),
                id: "rich-content".to_owned(),
                version: "1".to_owned(),
                artifact: "client/rich.zip".to_owned(),
                sha256: "a".repeat(64),
                size_bytes: 128,
                loaders: vec![
                    LoaderPlatform::Fabric,
                    LoaderPlatform::NeoForge,
                    LoaderPlatform::Forge,
                ],
                content: vec![
                    LoaderContentKind::Blocks,
                    LoaderContentKind::Items,
                    LoaderContentKind::Views,
                    LoaderContentKind::Assets,
                    LoaderContentKind::ViewActions,
                ],
                permissions: vec![
                    LoaderPermission::RegisterBlocks,
                    LoaderPermission::RegisterItems,
                    LoaderPermission::PresentViews,
                    LoaderPermission::LoadAssets,
                    LoaderPermission::SendViewActions,
                ],
                cache_key: format!("example:rich-content/1/{}", "a".repeat(64)),
                source_path: None,
                artifact_bytes: None,
                block_id: None,
                block_name: None,
            }],
        }
    }

    #[test]
    fn manifest_and_ack_round_trip_all_three_loader_platforms() {
        let manifest = manifest();
        let encoded = manifest.encode().unwrap();
        assert_eq!(
            serde_json::from_slice::<LoaderManifest>(&encoded).unwrap(),
            manifest
        );
        for platform in [
            LoaderPlatform::Fabric,
            LoaderPlatform::NeoForge,
            LoaderPlatform::Forge,
        ] {
            let ack = LoaderClientAck {
                protocol: LOADER_PROTOCOL_VERSION,
                platform,
                loader_version: "prototype".to_owned(),
                accepted_permissions: manifest.bundles[0].permissions.clone(),
                cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
                carrier_block_state_ids: BTreeMap::new(),
            };
            manifest.validate_ack(&ack).unwrap();
        }
    }

    #[test]
    fn ack_requires_permission_and_exact_cache_identity() {
        let manifest = manifest();
        let mut ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::NeoForge,
            loader_version: "26.1.2.76".to_owned(),
            accepted_permissions: Vec::new(),
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::new(),
        };
        assert!(matches!(
            manifest.validate_ack(&ack),
            Err(LoaderHandshakeError::PermissionDenied { .. })
        ));
        ack.accepted_permissions = manifest.bundles[0].permissions.clone();
        ack.cached_bundles.clear();
        assert!(matches!(
            manifest.validate_ack(&ack),
            Err(LoaderHandshakeError::BundleUnavailable { .. })
        ));
    }

    #[test]
    fn acknowledged_carrier_state_binds_only_to_the_verified_owner_block_identity() {
        let mut manifest = manifest();
        manifest.bundles[0].block_id = Some("example:ruby_block".to_owned());
        manifest.bundles[0].block_name = Some("Ruby Block".to_owned());
        let mut ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "26.1.2".to_owned(),
            accepted_permissions: manifest.bundles[0].permissions.clone(),
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::from([("example:ruby_block".to_owned(), 321)]),
        };

        let session = manifest.bind_ack(&ack).unwrap();
        assert_eq!(session.platform(), LoaderPlatform::Fabric);
        assert_eq!(session.loader_version(), "26.1.2");
        assert_eq!(session.block_state_id("example:ruby_block"), Some(321));
        assert_eq!(session.block_state_id("other:ruby_block"), None);
        let mut report = vec![mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        }];
        assert_eq!(
            manifest.append_world_block_report(&mut report).unwrap(),
            vec![1]
        );
        let blocks = mc_world::BlockRegistry::from_report(&report).unwrap();
        let projection = session.block_projection(&blocks).unwrap();
        let world_state = projection.world_state(mc_world::BlockStateId(1)).unwrap();
        assert_eq!(world_state.owner_block_id(), "example:ruby_block");
        assert_eq!(world_state.canonical_state_id(), mc_world::BlockStateId(1));
        assert_eq!(
            projection.project(mc_world::BlockStateId(1)),
            mc_world::BlockStateId(321)
        );
        assert_eq!(
            projection.project(mc_world::BlockStateId(42)),
            mc_world::BlockStateId(42)
        );
        assert_eq!(
            manifest.world_block_state("example", "example:ruby_block", &blocks),
            Some(mc_world::BlockStateId(1))
        );
        assert_eq!(
            manifest.world_block_state("other", "example:ruby_block", &blocks),
            None
        );
        let items = mc_data::items::ItemRegistry::from_report(&[mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:paper").unwrap(),
            protocol_id: 777,
        }]);
        assert_eq!(
            manifest.world_block_item("example", "example:ruby_block", 3, &items),
            Some(
                ItemStack::new(777, 3)
                    .with_custom_name("Ruby Block")
                    .with_item_model(Identifier::parse("solaris_loader:loader_block").unwrap())
            )
        );
        assert_eq!(
            manifest.world_block_item("other", "example:ruby_block", 1, &items),
            None
        );
        let mut missing_permission = manifest.clone();
        missing_permission.bundles[0]
            .permissions
            .retain(|permission| *permission != LoaderPermission::RegisterBlocks);
        assert_eq!(
            missing_permission.world_block_state("example", "example:ruby_block", &blocks),
            None
        );
        assert_eq!(
            missing_permission.world_block_item("example", "example:ruby_block", 1, &items),
            None
        );

        ack.carrier_block_state_ids.clear();
        assert_eq!(
            manifest.bind_ack(&ack),
            Err(LoaderHandshakeError::MissingBlockCarrierState)
        );
    }

    #[test]
    fn multiple_carriers_bind_project_and_present_by_exact_owner_identity() {
        let mut manifest = manifest();
        manifest.bundles[0].block_id = Some("example:ruby_block".to_owned());
        manifest.bundles[0].block_name = Some("Ruby Block".to_owned());
        let mut second = manifest.bundles[0].clone();
        second.owner = "other".to_owned();
        second.id = "sapphire-content".to_owned();
        second.cache_key = format!("other:sapphire-content/1/{}", "b".repeat(64));
        second.sha256 = "b".repeat(64);
        second.block_id = Some("other:sapphire_block".to_owned());
        second.block_name = Some("Sapphire Block".to_owned());
        manifest.bundles.push(second);
        let ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::NeoForge,
            loader_version: "26.1.2".to_owned(),
            accepted_permissions: manifest.bundles[0].permissions.clone(),
            cached_bundles: manifest
                .bundles
                .iter()
                .map(|bundle| bundle.cache_key.clone())
                .collect(),
            carrier_block_state_ids: BTreeMap::from([
                ("example:ruby_block".to_owned(), 321),
                ("other:sapphire_block".to_owned(), 654),
            ]),
        };

        let session = manifest.bind_ack(&ack).unwrap();
        let mut report = vec![mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        }];
        assert_eq!(
            manifest.append_world_block_report(&mut report).unwrap(),
            vec![1, 2]
        );
        let blocks = mc_world::BlockRegistry::from_report(&report).unwrap();
        let projection = session.block_projection(&blocks).unwrap();
        assert_eq!(
            projection.project(mc_world::BlockStateId(1)),
            mc_world::BlockStateId(321)
        );
        assert_eq!(
            projection.project(mc_world::BlockStateId(2)),
            mc_world::BlockStateId(654)
        );
        assert_eq!(
            projection.canonical_state_for_item_model(&loader_block_item_model(1)),
            Some(mc_world::BlockStateId(2))
        );
        let items = mc_data::items::ItemRegistry::from_report(&[mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:paper").unwrap(),
            protocol_id: 777,
        }]);
        assert_eq!(
            manifest
                .world_block_item("other", "other:sapphire_block", 1, &items)
                .unwrap()
                .item_model
                .as_deref(),
            Some(&loader_block_item_model(1))
        );

        let mut incomplete = ack.clone();
        incomplete
            .carrier_block_state_ids
            .remove("other:sapphire_block");
        assert_eq!(
            manifest.bind_ack(&incomplete),
            Err(LoaderHandshakeError::BlockCarrierSetMismatch)
        );
        let mut duplicate = ack;
        duplicate
            .carrier_block_state_ids
            .insert("other:sapphire_block".to_owned(), 321);
        assert_eq!(
            manifest.bind_ack(&duplicate),
            Err(LoaderHandshakeError::InvalidBlockCarrierState { state_id: 321 })
        );
    }

    #[test]
    fn artifact_index_supplies_one_exact_owner_block_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("block.bundle");
        let file = File::create(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(
                LOADER_ARTIFACT_INDEX_PATH,
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(
                br#"{"schema":2,"screens":[{"id":"example:showcase","kind":"settlement","title":"Showcase","widgets":[]}],"blocks":[{"id":"example:ruby_block","model":"example:block/ruby_block","name":"Ruby Block"}],"items":[],"assets":[]}"#,
            )
            .unwrap();
        archive.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let size = bytes.len() as u64;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));

        assert_eq!(
            read_block_from_artifact_bytes(&bytes, &path, "example", size, &sha256)
                .unwrap()
                .id,
            "example:ruby_block"
        );
        assert!(matches!(
            read_block_from_artifact_bytes(&bytes, &path, "other", size, &sha256),
            Err(LoaderHandshakeError::ArtifactIndex(_))
        ));
        assert!(matches!(
            read_block_from_artifact_bytes(&bytes, &path, "example", size, &"0".repeat(64)),
            Err(LoaderHandshakeError::ArtifactIndex(_))
        ));
    }

    #[test]
    fn exact_request_resolves_only_the_manifest_artifact() {
        let mut manifest = manifest();
        manifest.bundles[0].source_path = Some(PathBuf::from("/plugin/client/rich.zip"));
        manifest.bundles[0].artifact_bytes = Some(Arc::from(&b"verified artifact"[..]));
        let request = LoaderArtifactRequest {
            protocol: LOADER_PROTOCOL_VERSION,
            cache_key: manifest.bundles[0].cache_key.clone(),
        };

        let (bundle, bytes) = manifest.requested_artifact(&request).unwrap();

        assert_eq!(bundle.id, "rich-content");
        assert_eq!(bytes, b"verified artifact");
        let unknown = LoaderArtifactRequest {
            cache_key: format!("{}-other", request.cache_key),
            ..request
        };
        assert!(matches!(
            manifest.requested_artifact(&unknown),
            Err(LoaderHandshakeError::UnknownBundleRequest { .. })
        ));
    }

    #[test]
    fn artifact_chunk_encoding_is_bounded_and_big_endian() {
        let payload = encode_artifact_chunk("cache-key", 42, true, b"abc").unwrap();

        assert_eq!(&payload[0..2], &LOADER_PROTOCOL_VERSION.to_be_bytes());
        assert_eq!(&payload[2..4], &9_u16.to_be_bytes());
        assert_eq!(&payload[4..13], b"cache-key");
        assert_eq!(&payload[13..21], &42_u64.to_be_bytes());
        assert_eq!(payload[21], 1);
        assert_eq!(&payload[22..], b"abc");
        assert!(
            encode_artifact_chunk(
                "cache-key",
                0,
                false,
                &vec![0; LOADER_ARTIFACT_CHUNK_BYTES + 1],
            )
            .is_err()
        );
    }

    #[test]
    fn view_request_decoding_is_closed_and_protocol_fenced() {
        let action = br#"{"protocol":3,"message":"view_action","view_instance_id":"solaris:view-1","view_revision":2,"action_id":"example:place","action_sequence":1,"fields":[{"id":"count","number":4}],"selection_token":"example:ctx-1"}"#;
        let decoded = decode_loader_view_client_request(action).unwrap();
        let LoaderViewClientRequest::Action(action) = decoded else {
            panic!("expected a view action");
        };
        assert_eq!(action.view_instance_id, "solaris:view-1");
        assert_eq!(action.view_revision, 2);
        assert_eq!(action.action_sequence, 1);
        assert_eq!(action.selection_token.as_deref(), Some("example:ctx-1"));
        assert_eq!(action.fields.len(), 1);

        let unknown = br#"{"protocol":3,"message":"view_action","view_instance_id":"a:b","view_revision":1,"action_id":"a:b","action_sequence":1,"price":9}"#;
        assert!(decode_loader_view_client_request(unknown).is_err());
        let wrong_protocol =
            br#"{"protocol":2,"message":"view_request","request_kind":"settlement"}"#;
        assert!(matches!(
            decode_loader_view_client_request(wrong_protocol),
            Err(LoaderHandshakeError::Protocol { .. })
        ));
        let request = br#"{"protocol":3,"message":"view_request","request_kind":"army"}"#;
        assert_eq!(
            decode_loader_view_client_request(request).unwrap(),
            LoaderViewClientRequest::ViewRequest {
                request_kind: mc_script::ScriptClientViewRequestKind::Army,
            }
        );
        let cancel = br#"{"protocol":3,"message":"cancel_selection","selection_context_id":"example:ctx-1"}"#;
        assert_eq!(
            decode_loader_view_client_request(cancel).unwrap(),
            LoaderViewClientRequest::CancelSelection {
                selection_context_id: "example:ctx-1".to_owned(),
            }
        );
    }

    #[test]
    fn view_message_encoding_matches_the_wire_3_model() {
        let model = mc_script::ScriptClientViewModel::try_new(
            0,
            1,
            vec![mc_script::ScriptClientViewRow::try_new(vec!["Hamlet".to_owned()]).unwrap()],
            vec![mc_script::ScriptClientViewField::try_number("count", 4.0).unwrap()],
            vec![
                mc_script::ScriptClientViewAction::try_new("example:place", true, None, None)
                    .unwrap(),
            ],
            Vec::new(),
            Vec::new(),
            vec![
                mc_script::ScriptClientViewMarker::try_new("anchor", None, None, None, None)
                    .unwrap(),
            ],
            None,
        )
        .unwrap();
        let payload = encode_loader_view_message(&LoaderViewWireMessage::Open {
            view_instance_id: "solaris:view-1",
            revision: 1,
            view_id: "example:showcase",
            title: "Showcase",
            model: &model,
        })
        .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(document["protocol"], 3);
        assert_eq!(document["message"], "open_view");
        assert_eq!(document["model"]["rows"][0]["cells"][0], "Hamlet");
        assert_eq!(document["model"]["fields"][0]["number"], 4.0);
        assert_eq!(
            document["model"]["actions"][0]["action_id"],
            "example:place"
        );
        assert_eq!(document["model"]["markers"][0]["marker_id"], "anchor");
        assert!(
            document["model"]["markers"][0]
                .get("selection_token")
                .is_none()
        );
    }
}
