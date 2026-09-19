//! The `[client]` section of `plugin.toml`, for every package the host loads.
//!
//! A package that ships client content declares it here: one entry per bundle,
//! each naming the loaders it supports, the content kinds it carries, the
//! permissions those kinds need, and the artifact's own path, size and hash. The
//! host validates the whole declaration and reads the artifact itself, so a
//! bundle reaches the Loader as bytes this host hashed rather than as a claim a
//! package made.
//!
//! This parser is the component deployment's single client-content path. An
//! artifact is accepted, hashed and staged only after its declaration validates;
//! the result is `mc_script`'s canonical [`ClientBundle`].

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_script::{ClientBundle, ClientContentKind, ClientLoader, ClientPermission};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The only client manifest schema this host reads.
const CLIENT_MANIFEST_SCHEMA: u16 = 2;
/// Bundles one package may ship.
const MAX_CLIENT_BUNDLES_PER_PLUGIN: usize = 8;
/// Longest accepted bundle id.
const MAX_CLIENT_BUNDLE_ID_BYTES: usize = 48;
/// Longest accepted bundle version.
const MAX_CLIENT_BUNDLE_VERSION_BYTES: usize = 32;
/// Longest accepted artifact path.
const MAX_CLIENT_ARTIFACT_PATH_BYTES: usize = 160;
/// Largest artifact one bundle may declare.
const MAX_CLIENT_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;

/// The `[client]` section of a package manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskClient {
    schema: u16,
    #[serde(default)]
    bundles: Vec<DiskClientBundle>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskClientBundle {
    id: String,
    version: String,
    artifact: String,
    sha256: String,
    size_bytes: u64,
    loaders: Vec<DiskClientLoader>,
    content: Vec<DiskClientContentKind>,
    permissions: Vec<DiskClientPermission>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskClientLoader {
    Fabric,
    #[serde(rename = "neoforge")]
    NeoForge,
    Forge,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskClientContentKind {
    Blocks,
    Items,
    Views,
    ViewActions,
    Assets,
    WorldPreviews,
    WorldSelection,
    Sounds,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskClientPermission {
    RegisterBlocks,
    RegisterItems,
    PresentViews,
    SendViewActions,
    LoadAssets,
    PresentWorldPreviews,
    SendWorldSelection,
    PlaySounds,
}

/// Materialize one package's client bundles into the canonical descriptors.
///
/// Absent `[client]` means a server-only package and answers no bundles. A
/// declared section is bounded, duplicate-free, and every content kind carries the
/// permission it needs; each artifact is then read from inside the package
/// directory and must match its declared size and SHA-256 exactly.
pub fn materialize_client_bundles(
    plugin_directory: &Path,
    owner_plugin_id: &str,
    client: Option<DiskClient>,
) -> Result<Vec<ClientBundle>, String> {
    let Some(client) = client else {
        return Ok(Vec::new());
    };
    if client.schema != CLIENT_MANIFEST_SCHEMA {
        return Err(format!(
            "client manifest schema must be {CLIENT_MANIFEST_SCHEMA}, got {}",
            client.schema
        ));
    }
    if client.bundles.is_empty() {
        return Err("client manifest must declare at least one bundle".to_owned());
    }
    if client.bundles.len() > MAX_CLIENT_BUNDLES_PER_PLUGIN {
        return Err(format!(
            "client bundles exceed {MAX_CLIENT_BUNDLES_PER_PLUGIN} entries"
        ));
    }

    let mut bundle_ids = HashSet::new();
    let mut bundles = Vec::with_capacity(client.bundles.len());
    for bundle in client.bundles {
        validate_client_literal(&bundle.id, "client bundle id", MAX_CLIENT_BUNDLE_ID_BYTES)?;
        if !bundle_ids.insert(bundle.id.clone()) {
            return Err(format!("duplicate client bundle id {:?}", bundle.id));
        }
        validate_client_literal(
            &bundle.version,
            "client bundle version",
            MAX_CLIENT_BUNDLE_VERSION_BYTES,
        )?;
        validate_client_artifact_path(&bundle.artifact)?;
        if bundle.sha256.len() != 64
            || !bundle
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "client bundle {:?} sha256 must be 64 lowercase hexadecimal characters",
                bundle.id
            ));
        }
        if bundle.size_bytes == 0 || bundle.size_bytes > MAX_CLIENT_BUNDLE_BYTES {
            return Err(format!(
                "client bundle {:?} size_bytes must be 1..={MAX_CLIENT_BUNDLE_BYTES}",
                bundle.id
            ));
        }

        let loaders = unique_client_values(
            bundle.loaders.into_iter().map(|loader| match loader {
                DiskClientLoader::Fabric => ClientLoader::Fabric,
                DiskClientLoader::NeoForge => ClientLoader::NeoForge,
                DiskClientLoader::Forge => ClientLoader::Forge,
            }),
            "loaders",
            &bundle.id,
        )?;
        let content = unique_client_values(
            bundle.content.into_iter().map(|content| match content {
                DiskClientContentKind::Blocks => ClientContentKind::Blocks,
                DiskClientContentKind::Items => ClientContentKind::Items,
                DiskClientContentKind::Views => ClientContentKind::Views,
                DiskClientContentKind::ViewActions => ClientContentKind::ViewActions,
                DiskClientContentKind::Assets => ClientContentKind::Assets,
                DiskClientContentKind::WorldPreviews => ClientContentKind::WorldPreviews,
                DiskClientContentKind::WorldSelection => ClientContentKind::WorldSelection,
                DiskClientContentKind::Sounds => ClientContentKind::Sounds,
            }),
            "content",
            &bundle.id,
        )?;
        let permissions = unique_client_values(
            bundle
                .permissions
                .into_iter()
                .map(|permission| match permission {
                    DiskClientPermission::RegisterBlocks => ClientPermission::RegisterBlocks,
                    DiskClientPermission::RegisterItems => ClientPermission::RegisterItems,
                    DiskClientPermission::PresentViews => ClientPermission::PresentViews,
                    DiskClientPermission::SendViewActions => ClientPermission::SendViewActions,
                    DiskClientPermission::LoadAssets => ClientPermission::LoadAssets,
                    DiskClientPermission::PresentWorldPreviews => {
                        ClientPermission::PresentWorldPreviews
                    }
                    DiskClientPermission::SendWorldSelection => {
                        ClientPermission::SendWorldSelection
                    }
                    DiskClientPermission::PlaySounds => ClientPermission::PlaySounds,
                }),
            "permissions",
            &bundle.id,
        )?;
        for content_kind in &content {
            let required = content_kind.required_permission();
            if !permissions.contains(&required) {
                return Err(format!(
                    "client bundle {:?} content {:?} requires permission {:?}",
                    bundle.id,
                    content_kind.contract_name(),
                    required.contract_name()
                ));
            }
        }
        let (artifact_path, artifact_bytes) = validate_client_artifact(
            plugin_directory,
            &bundle.artifact,
            bundle.size_bytes,
            &bundle.sha256,
        )?;

        bundles.push(ClientBundle::new(
            owner_plugin_id,
            bundle.id,
            bundle.version,
            bundle.artifact,
            bundle.sha256,
            bundle.size_bytes,
            artifact_path,
            artifact_bytes,
            loaders,
            content,
            permissions,
        ));
    }
    Ok(bundles)
}

fn validate_client_artifact(
    plugin_directory: &Path,
    relative_path: &str,
    declared_size: u64,
    declared_sha256: &str,
) -> Result<(PathBuf, Arc<[u8]>), String> {
    let plugin_root = fs::canonicalize(plugin_directory).map_err(|error| {
        format!(
            "canonicalizing plugin directory {}: {error}",
            plugin_directory.display()
        )
    })?;
    let artifact_path = plugin_directory.join(relative_path);
    let canonical = fs::canonicalize(&artifact_path).map_err(|error| {
        format!(
            "opening client artifact {}: {error}",
            artifact_path.display()
        )
    })?;
    if !canonical.starts_with(&plugin_root) {
        return Err(format!(
            "client artifact {} escapes the plugin directory",
            artifact_path.display()
        ));
    }
    let mut file = fs::File::open(&canonical).map_err(|error| {
        format!(
            "opening client artifact {}: {error}",
            artifact_path.display()
        )
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("reading client artifact metadata: {error}"))?;
    if !metadata.is_file() {
        return Err(format!(
            "client artifact {} is not a regular file",
            artifact_path.display()
        ));
    }
    if metadata.len() != declared_size {
        return Err(format!(
            "client artifact {} has {} bytes, manifest declares {declared_size}",
            artifact_path.display(),
            metadata.len()
        ));
    }
    let byte_len = usize::try_from(declared_size).map_err(|_| {
        format!(
            "client artifact {} is too large for this platform",
            artifact_path.display()
        )
    })?;
    let mut bytes = vec![0_u8; byte_len];
    file.read_exact(&mut bytes).map_err(|error| {
        format!(
            "reading client artifact {}: {error}",
            artifact_path.display()
        )
    })?;
    let mut trailing = [0_u8; 1];
    if file.read(&mut trailing).map_err(|error| {
        format!(
            "checking client artifact {} length: {error}",
            artifact_path.display()
        )
    })? != 0
    {
        return Err(format!(
            "client artifact {} changed while reading",
            artifact_path.display()
        ));
    }
    let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if actual_sha256 != declared_sha256 {
        return Err(format!(
            "client artifact {} SHA-256 does not match the manifest",
            artifact_path.display()
        ));
    }
    Ok((canonical, Arc::<[u8]>::from(bytes)))
}

fn unique_client_values<T>(
    values: impl IntoIterator<Item = T>,
    field: &str,
    bundle_id: &str,
) -> Result<Vec<T>, String>
where
    T: Copy + Eq + std::hash::Hash,
{
    let mut unique = HashSet::new();
    let mut result = Vec::new();
    for value in values {
        if !unique.insert(value) {
            return Err(format!(
                "client bundle {bundle_id:?} contains duplicate {field}"
            ));
        }
        result.push(value);
    }
    if result.is_empty() {
        return Err(format!(
            "client bundle {bundle_id:?} must declare at least one {field}"
        ));
    }
    Ok(result)
}

fn validate_client_literal(value: &str, field: &str, max_bytes: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(format!("{field} must contain 1..={max_bytes} bytes"));
    }
    if matches!(value, "." | "..") {
        return Err(format!("{field} cannot be a relative path segment"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        return Err(format!("{field} {value:?} contains invalid characters"));
    }
    Ok(())
}

fn validate_client_artifact_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > MAX_CLIENT_ARTIFACT_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./-".contains(&byte))
    {
        return Err(format!(
            "client artifact path {path:?} must be a bounded relative ASCII path"
        ));
    }
    Ok(())
}
