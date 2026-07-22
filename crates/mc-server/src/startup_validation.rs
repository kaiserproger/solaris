use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use mc_server::ServerConfig;

pub(crate) const WORLD_CONTRACT_SCHEMA: u32 = 1;
const WORLD_CONTRACT_FILE: &str = "world.json";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct PersistedWorldContract {
    pub(crate) schema: u32,
    pub(crate) worldgen_revision: u32,
    pub(crate) seed: i64,
    pub(crate) mode: String,
    pub(crate) min_y: i32,
    pub(crate) height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorldSource {
    SolarisGenerated,
    ExistingVanilla,
}

pub(crate) fn required_world_dir(config: &ServerConfig) -> Result<&Path> {
    let Some(world_dir) = config.data.world_dir.as_deref() else {
        bail!("data.world_dir is required to start a playable persistent server");
    };
    match std::fs::metadata(world_dir) {
        Ok(metadata) if !metadata.is_dir() => {
            bail!("data.world_dir is not a directory: {}", world_dir.display());
        }
        Ok(_) if world_region_root_is_blocked(world_dir) => {
            bail!(
                "data.world_dir region path is not a directory: {}",
                world_dir.join("region").display()
            );
        }
        Ok(_) => {}
        Err(_) if has_non_directory_ancestor(world_dir) => {
            bail!(
                "data.world_dir has a non-directory parent: {}",
                world_dir.display()
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "reading data.world_dir metadata for {}",
                    world_dir.display()
                )
            });
        }
    }
    Ok(world_dir)
}

pub(crate) fn validate_runtime_config(config: &ServerConfig) -> Result<()> {
    required_world_dir(config)?;
    config.data.chunk_geometry().map_err(anyhow::Error::msg)?;
    if !(mc_net::MIN_VIEW_DISTANCE..=mc_net::MAX_VIEW_DISTANCE)
        .contains(&config.server.view_distance)
    {
        bail!(
            "server.view_distance must be between {} and {} (inclusive)",
            mc_net::MIN_VIEW_DISTANCE,
            mc_net::MAX_VIEW_DISTANCE,
        );
    }
    if !(mc_net::MIN_VIEW_DISTANCE..=mc_net::MAX_VIEW_DISTANCE)
        .contains(&config.server.simulation_distance)
    {
        bail!(
            "server.simulation_distance must be between {} and {} (inclusive)",
            mc_net::MIN_VIEW_DISTANCE,
            mc_net::MAX_VIEW_DISTANCE,
        );
    }
    if config.simulation.save_interval_ticks == 0 {
        bail!("simulation.save_interval_ticks must be greater than 0");
    }
    if let Some(vanilla_data_dir) = config.data.vanilla_data_dir.as_deref() {
        validate_vanilla_sidecar_version(vanilla_data_dir)?;
    }
    Ok(())
}

pub(crate) fn world_contract_path(world_dir: &Path) -> PathBuf {
    world_dir.join("solaris").join(WORLD_CONTRACT_FILE)
}

pub(crate) fn ensure_world_contract(
    world_dir: &Path,
    configured: mc_world::ChunkGeometry,
    seed: i64,
    mode: &str,
) -> Result<WorldSource> {
    let path = world_contract_path(world_dir);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let persisted: PersistedWorldContract = serde_json::from_slice(&bytes)
                .with_context(|| format!("reading persisted world contract {}", path.display()))?;
            if persisted.schema != WORLD_CONTRACT_SCHEMA {
                bail!(
                    "unsupported persisted world contract schema {} in {}; expected {}",
                    persisted.schema,
                    path.display(),
                    WORLD_CONTRACT_SCHEMA,
                );
            }
            if persisted.worldgen_revision != mc_worldgen::WORLDGEN_REVISION
                || persisted.seed != seed
                || persisted.mode != mode
            {
                bail!(
                    "persisted worldgen revision={} seed={} mode={} in {} does not match configured revision={} seed={} mode={}; use a fresh world_dir",
                    persisted.worldgen_revision,
                    persisted.seed,
                    persisted.mode,
                    path.display(),
                    mc_worldgen::WORLDGEN_REVISION,
                    seed,
                    mode,
                );
            }
            let stored = mc_world::ChunkGeometry::new(persisted.min_y, persisted.height)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid persisted world contract geometry min_y={} height={} in {}",
                        persisted.min_y,
                        persisted.height,
                        path.display(),
                    )
                })?;
            if stored != configured {
                bail!(
                    "persisted world contract geometry {}..{} in {} does not match configured geometry {}..{}",
                    stored.min_y(),
                    stored.max_y(),
                    path.display(),
                    configured.min_y(),
                    configured.max_y(),
                );
            }
            Ok(WorldSource::SolarisGenerated)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if world_contains_anvil_data(world_dir)? {
                return Ok(WorldSource::ExistingVanilla);
            }
            write_world_contract(&path, configured, seed, mode)?;
            Ok(WorldSource::SolarisGenerated)
        }
        Err(error) => Err(error)
            .with_context(|| format!("reading persisted world contract {}", path.display())),
    }
}

fn world_contains_anvil_data(world_dir: &Path) -> Result<bool> {
    for region_dir in [
        world_dir.join("dimensions/minecraft/overworld/region"),
        world_dir.join("region"),
    ] {
        let entries = match std::fs::read_dir(&region_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("checking existing Anvil data in {}", region_dir.display())
                });
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| {
                format!("checking existing Anvil data in {}", region_dir.display())
            })?;
            let file_type = entry
                .file_type()
                .with_context(|| format!("checking Anvil entry {}", entry.path().display()))?;
            if file_type.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mca"))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn write_world_contract(
    path: &Path,
    geometry: mc_world::ChunkGeometry,
    seed: i64,
    mode: &str,
) -> Result<()> {
    let parent = path
        .parent()
        .expect("world geometry contract path has a parent");
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating Solaris metadata directory {}", parent.display()))?;
    let metadata = PersistedWorldContract {
        schema: WORLD_CONTRACT_SCHEMA,
        worldgen_revision: mc_worldgen::WORLDGEN_REVISION,
        seed,
        mode: mode.to_owned(),
        min_y: geometry.min_y(),
        height: geometry.height(),
    };
    let bytes =
        serde_json::to_vec_pretty(&metadata).context("encoding persisted world contract")?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| {
            format!(
                "creating temporary world geometry contract {}",
                temporary.display()
            )
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .with_context(|| {
            format!(
                "writing temporary world geometry contract {}",
                temporary.display()
            )
        })?;
    std::fs::rename(&temporary, path).with_context(|| {
        format!(
            "installing persisted world contract {} from {}",
            path.display(),
            temporary.display()
        )
    })?;
    sync_metadata_directory(parent)
}

#[cfg(unix)]
fn sync_metadata_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("syncing Solaris metadata directory {}", path.display()))
}

#[cfg(not(unix))]
fn sync_metadata_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[derive(serde::Deserialize)]
struct VanillaVersionMetadata {
    id: String,
    world_version: u32,
    protocol_version: i32,
}

pub(crate) fn validate_vanilla_sidecar_version(vanilla_data_dir: &Path) -> Result<()> {
    let metadata = std::fs::metadata(vanilla_data_dir).with_context(|| {
        format!(
            "reading vanilla sidecar directory metadata for {}",
            vanilla_data_dir.display()
        )
    })?;
    if !metadata.is_dir() {
        bail!(
            "data.vanilla_data_dir is not a directory: {}",
            vanilla_data_dir.display()
        );
    }

    let path = vanilla_data_dir.join("version.json");
    let raw = std::fs::read(&path)
        .with_context(|| format!("reading vanilla sidecar version from {}", path.display()))?;
    let version = serde_json::from_slice::<VanillaVersionMetadata>(&raw)
        .with_context(|| format!("parsing vanilla sidecar version from {}", path.display()))?;

    if version.id != mc_protocol::TARGET_RELEASE {
        bail!(
            "vanilla sidecar release id {:?} does not match Solaris target {:?}",
            version.id,
            mc_protocol::TARGET_RELEASE
        );
    }
    if version.world_version != mc_protocol::WORLD_VERSION {
        bail!(
            "vanilla sidecar world_version {} does not match Solaris world version {}",
            version.world_version,
            mc_protocol::WORLD_VERSION
        );
    }
    if version.protocol_version != mc_protocol::PROTOCOL_VERSION {
        bail!(
            "vanilla sidecar protocol_version {} does not match Solaris protocol version {}",
            version.protocol_version,
            mc_protocol::PROTOCOL_VERSION
        );
    }

    Ok(())
}

pub(crate) fn has_non_directory_ancestor(path: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .find_map(|ancestor| match std::fs::metadata(ancestor) {
            Ok(metadata) => Some(!metadata.is_dir()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some(false),
        })
        .unwrap_or(false)
}

pub(crate) fn world_region_root_is_blocked(world_dir: &Path) -> bool {
    let modern = world_dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region");
    let legacy = world_dir.join("region");
    if modern.is_dir() || legacy.is_dir() {
        return false;
    }
    std::fs::metadata(legacy).is_ok_and(|metadata| !metadata.is_dir())
}

pub(crate) fn is_public_bind_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_private() && !ip.is_link_local(),
        IpAddr::V6(ip) => !ip.is_loopback() && !ip.is_unique_local(),
    }
}
