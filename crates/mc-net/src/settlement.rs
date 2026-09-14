//! Package-driven settlement catalog discovery and the live world adapter.
//!
//! A deployed package opts into the settlement profile by declaring both the
//! `world_sites` and `structure_operations` required features *and* shipping an
//! authored `structures/` catalog next to its manifest. Startup reads that
//! catalog once, validates it through the `mc-worldgen` blueprint loader (closed
//! schema, owner namespace, frozen limits, derived content hash), and fails
//! loudly on any violation instead of degrading to an empty catalog. A world
//! with no such package keeps the config-driven prototype path and every
//! settlement call answers the typed `runtime_unavailable`.
//!
//! [`LiveSettlementWorld`] is the only production [`SettlementWorld`]: it
//! surveys resident terrain, localizes durable footprint changes through the
//! world chunk journal watermark, refuses foreign protected zones, and commits
//! every structure portion through the same world storage kernel a player edit
//! uses, inside the documented 512-block portion bound.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mc_script::{
    LuaPluginPackage, MAX_WORLD_COMMIT_PORTION, ScriptChunkAvailability, ScriptOperationFailure,
    ScriptSurveyBounds,
};
use mc_world::{BlockPos, BlockRegistry, Chunk, ChunkPos, WorldReadView};
use mc_worldgen::{BlueprintCatalog, CatalogError, SettlementSelector};
use sha2::{Digest, Sha256};

use crate::play::BlockEdit;
use crate::play::SimulationHandle;
use crate::play::owned_inventory::container_slot_to_item;
use crate::play::owned_inventory::{WarehouseTransferOutcome, WarehouseTransferRequest};
use crate::script::PluginZoneAdapter;
use crate::script::storage::{
    ContainerReading, SettlementRuntime, SettlementWorld, StructureBlockPlacement, SurveyReading,
};

/// Manifest feature that owns settlement site discovery.
const WORLD_SITES_FEATURE: &str = "world_sites";
/// Manifest feature that owns staged construction.
const STRUCTURE_OPERATIONS_FEATURE: &str = "structure_operations";
/// Authored catalog directory inside a deployed package.
const STRUCTURES_DIRECTORY: &str = "structures";
/// The only dimension the v1 settlement profile surveys.
const OVERWORLD: &str = "minecraft:overworld";
/// Local block axis of one chunk.
const CHUNK_AXIS: i32 = 16;
/// Deepest water column the survey reports.
const MAX_WATER_DEPTH: u8 = u8::MAX;
/// Footprint observations retained before an unseen revision fails closed.
const MAX_OBSERVED_REVISIONS: usize = 65_536;
/// Placeholder biome used when a section carries no biome palette.
const FALLBACK_BIOME: &str = "minecraft:plains";

/// Everything that can make the deployed settlement catalog unusable.
#[derive(Debug)]
pub(crate) enum SettlementStartupError {
    /// Two deployed packages claim the same settlement profile.
    DuplicateProfile { first: String, second: String },
    /// A package claims the profile but ships no authored catalog directory.
    MissingCatalog { plugin_id: String, path: PathBuf },
    /// The catalog directory could not be listed or a file could not be read.
    UnreadableCatalog {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The catalog directory holds something that is not an authored blueprint.
    StrayCatalogEntry {
        plugin_id: String,
        path: PathBuf,
        reason: &'static str,
    },
    /// The blueprint loader rejected the catalog (limits, schema, hash, ...).
    Catalog {
        plugin_id: String,
        source: CatalogError,
    },
    /// The catalog provides no blueprint for a role every variant needs.
    MissingRole {
        plugin_id: String,
        role: &'static str,
    },
}

impl fmt::Display for SettlementStartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProfile { first, second } => write!(
                formatter,
                "settlement profile is claimed by both `{first}` and `{second}`; a deployed set holds at most one settlement profile"
            ),
            Self::MissingCatalog { plugin_id, path } => write!(
                formatter,
                "settlement package `{plugin_id}` has no authored catalog directory {}",
                path.display()
            ),
            Self::UnreadableCatalog { path, source } => {
                write!(
                    formatter,
                    "reading settlement catalog {}: {source}",
                    path.display()
                )
            }
            Self::StrayCatalogEntry {
                plugin_id,
                path,
                reason,
            } => write!(
                formatter,
                "settlement package `{plugin_id}` catalog entry {} is not an authored blueprint: {reason}",
                path.display()
            ),
            Self::Catalog { plugin_id, source } => write!(
                formatter,
                "settlement package `{plugin_id}` blueprint catalog rejected: {source}"
            ),
            Self::MissingRole { plugin_id, role } => write!(
                formatter,
                "settlement package `{plugin_id}` catalog provides no {role} blueprint; every variant needs one"
            ),
        }
    }
}

impl std::error::Error for SettlementStartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnreadableCatalog { source, .. } => Some(source),
            Self::Catalog { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// One validated deployed settlement catalog.
pub(crate) struct SettlementDeployment {
    plugin_id: String,
    profile_revision: u64,
    catalog: Arc<BlueprintCatalog>,
}

impl fmt::Debug for SettlementDeployment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SettlementDeployment")
            .field("plugin_id", &self.plugin_id)
            .field("profile_revision", &self.profile_revision)
            .field("blueprints", &self.catalog.len())
            .finish()
    }
}

impl SettlementDeployment {
    pub(crate) fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub(crate) fn profile_revision(&self) -> u64 {
        self.profile_revision
    }

    /// Build the deterministic runtime for one world.
    ///
    /// `ground` is the terrain generator the world itself generates from, so a
    /// settlement's base rows come from the terrain players stand on.
    pub(crate) fn runtime(
        &self,
        seed: i64,
        world_identity: &str,
        start_cell: [i32; 2],
        ground: Arc<dyn mc_world::ChunkGenerator>,
    ) -> SettlementRuntime {
        SettlementRuntime::new(
            SettlementSelector::new(seed, self.profile_revision),
            Arc::clone(&self.catalog),
            world_identity,
            start_cell,
            ground,
        )
    }
}

/// Find the deployed package that owns the settlement profile and validate its
/// authored catalog, or `None` when no package claims the profile.
///
/// A violation is returned as a typed error; the caller must fail startup.
pub(crate) fn discover_settlement_deployment(
    packages: &[LuaPluginPackage],
    registry: &BlockRegistry,
) -> Result<Option<SettlementDeployment>, SettlementStartupError> {
    discover_settlement_deployment_with_hashes(packages, registry, &BTreeMap::new())
}

/// As [`discover_settlement_deployment`], additionally pinning blueprints whose
/// content hash the deployment records. A mismatch is rejected like any other
/// catalog violation.
pub(crate) fn discover_settlement_deployment_with_hashes(
    packages: &[LuaPluginPackage],
    registry: &BlockRegistry,
    expected_hashes: &BTreeMap<String, String>,
) -> Result<Option<SettlementDeployment>, SettlementStartupError> {
    let mut owners = packages.iter().filter(|package| {
        package.declares_feature(WORLD_SITES_FEATURE)
            && package.declares_feature(STRUCTURE_OPERATIONS_FEATURE)
    });
    let Some(owner) = owners.next() else {
        return Ok(None);
    };
    if let Some(second) = owners.next() {
        return Err(SettlementStartupError::DuplicateProfile {
            first: owner.plugin_id().to_owned(),
            second: second.plugin_id().to_owned(),
        });
    }
    let directory = owner.package_dir().join(STRUCTURES_DIRECTORY);
    if !directory.is_dir() {
        return Err(SettlementStartupError::MissingCatalog {
            plugin_id: owner.plugin_id().to_owned(),
            path: directory,
        });
    }
    let files = read_blueprint_files(owner.plugin_id(), &directory)?;
    // Authored ids use the package's content namespace, which is not always the
    // package id (`solaris-settlements` ships `solaris:*` blueprints). The
    // namespace is taken from the catalog itself and every file must agree with
    // it, so the loader still rejects a mixed or foreign namespace.
    let namespace = catalog_namespace(&files).unwrap_or_else(|| owner.plugin_id().to_owned());
    let catalog =
        BlueprintCatalog::from_files_with_hashes(registry, &namespace, &files, expected_hashes)
            .map_err(|source| SettlementStartupError::Catalog {
                plugin_id: owner.plugin_id().to_owned(),
                source,
            })?;
    // A catalog missing a role can never lay out a settlement, so it fails
    // startup here instead of answering every later request with an unavailable
    // runtime.
    if let Some(role) = mc_worldgen::settlement_sites::missing_required_role(&catalog) {
        return Err(SettlementStartupError::MissingRole {
            plugin_id: owner.plugin_id().to_owned(),
            role: role.as_str(),
        });
    }
    Ok(Some(SettlementDeployment {
        plugin_id: owner.plugin_id().to_owned(),
        profile_revision: catalog_profile_revision(&catalog),
        catalog: Arc::new(catalog),
    }))
}

/// Read one package's authored catalog directory, rejecting anything that is not
/// a plain `.toml` file so a stray entry can never silently change the profile.
fn read_blueprint_files(
    plugin_id: &str,
    directory: &Path,
) -> Result<Vec<(String, String)>, SettlementStartupError> {
    let entries =
        fs::read_dir(directory).map_err(|source| SettlementStartupError::UnreadableCatalog {
            path: directory.to_path_buf(),
            source,
        })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| SettlementStartupError::UnreadableCatalog {
            path: directory.to_path_buf(),
            source,
        })?;
        paths.push(entry.path());
    }
    // Deterministic file order keeps the derived profile revision stable.
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata =
            fs::metadata(&path).map_err(|source| SettlementStartupError::UnreadableCatalog {
                path: path.clone(),
                source,
            })?;
        if !metadata.is_file() {
            return Err(SettlementStartupError::StrayCatalogEntry {
                plugin_id: plugin_id.to_owned(),
                path,
                reason: "not a regular file",
            });
        }
        if path.extension().is_none_or(|extension| extension != "toml") {
            return Err(SettlementStartupError::StrayCatalogEntry {
                plugin_id: plugin_id.to_owned(),
                path,
                reason: "not a .toml blueprint",
            });
        }
        let text = fs::read_to_string(&path).map_err(|source| {
            SettlementStartupError::UnreadableCatalog {
                path: path.clone(),
                source,
            }
        })?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        files.push((name, text));
    }
    Ok(files)
}

/// The content namespace every authored blueprint id declares.
///
/// The first namespaced `id = "ns:name"` in file order wins; stage and variant
/// ids never carry a namespace, so they cannot select it. A catalog that mixes
/// namespaces still fails inside the loader against this owner.
fn catalog_namespace(files: &[(String, String)]) -> Option<String> {
    files.iter().find_map(|(_, text)| {
        text.lines().find_map(|line| {
            let rest = line.trim().strip_prefix("id")?.trim_start();
            let rest = rest.strip_prefix('=')?.trim_start();
            let rest = rest.strip_prefix('"')?;
            let (value, _) = rest.split_once('"')?;
            value
                .split_once(':')
                .map(|(namespace, _)| namespace.to_owned())
        })
    })
}

/// Deterministic profile revision of a validated catalog.
///
/// Selection is a pure function of `(seed, profile revision, coordinates)`, so
/// the revision must change exactly when the authored profile changes: it is
/// derived from every blueprint's owned id, revision and derived content hash.
fn catalog_profile_revision(catalog: &BlueprintCatalog) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"solaris.settlement.profile.v1");
    for blueprint in catalog.blueprints() {
        hasher.update(blueprint.id().as_bytes());
        hasher.update([0]);
        hasher.update(blueprint.revision().to_le_bytes());
        hasher.update(blueprint.content_hash().as_bytes());
        hasher.update([0xff]);
    }
    let digest = hasher.finalize();
    let mut revision = [0_u8; 8];
    revision.copy_from_slice(&digest[..8]);
    let revision = u64::from_le_bytes(revision);
    // Zero is reserved for "no profile revision observed yet".
    revision.max(1)
}

/// The production [`SettlementWorld`] over live world storage.
pub(crate) struct LiveSettlementWorld {
    read: WorldReadView,
    blocks: Arc<BlockRegistry>,
    zones: Option<PluginZoneAdapter>,
    /// The single server-owned simulation handle structure portions commit
    /// through, owned by the same server that constructs this world.
    simulation: SimulationHandle,
    /// Next observation revision.
    revision: AtomicU64,
    /// Observed revision to the block content of the observed footprint.
    observations: Mutex<BTreeMap<u64, Option<u64>>>,
}

impl LiveSettlementWorld {
    pub(crate) fn new(
        read: WorldReadView,
        blocks: Arc<BlockRegistry>,
        zones: Option<PluginZoneAdapter>,
        simulation: SimulationHandle,
    ) -> Self {
        Self {
            read,
            blocks,
            zones,
            simulation,
            revision: AtomicU64::new(0),
            observations: Mutex::new(BTreeMap::new()),
        }
    }

    /// Mint a fresh observation revision over `bounds`.
    ///
    /// The revision records the block content of the observed volume, so a
    /// later [`SettlementWorld::footprint_changed_since`] check localizes a
    /// change to the footprint itself instead of to any durable write that
    /// happened to share a chunk with it.
    fn next_revision(&self, bounds: ScriptSurveyBounds) -> u64 {
        let revision = self
            .revision
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let footprint = self.footprint_digest(bounds);
        if let Ok(mut observations) = self.observations.lock() {
            if observations.len() >= MAX_OBSERVED_REVISIONS {
                observations.clear();
            }
            observations.insert(revision, footprint);
        }
        revision
    }

    /// A digest of every block state inside `bounds`, in a fixed order.
    ///
    /// `None` when any covering chunk is not loaded: a footprint that cannot be
    /// read back cannot be proven unchanged.
    fn footprint_digest(&self, bounds: ScriptSurveyBounds) -> Option<u64> {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        let positions = Self::chunk_positions(bounds);
        let snapshot = self.read.snapshot_chunks(&positions);
        let mut digest = FNV_OFFSET;
        for y in bounds.min[1]..=bounds.max[1] {
            for z in bounds.min[2]..=bounds.max[2] {
                for x in bounds.min[0]..=bounds.max[0] {
                    let position = ChunkPos {
                        x: x.div_euclid(CHUNK_AXIS),
                        z: z.div_euclid(CHUNK_AXIS),
                    };
                    let chunk = snapshot.chunk_ref(position)?;
                    let state = chunk
                        .get_block(
                            x.rem_euclid(CHUNK_AXIS) as u8,
                            y,
                            z.rem_euclid(CHUNK_AXIS) as u8,
                        )
                        .map_or(u64::MAX, |state| u64::from(state.0));
                    digest = (digest ^ state).wrapping_mul(FNV_PRIME);
                }
            }
        }
        Some(digest)
    }

    /// Chunk positions covering an inclusive block bounds rectangle.
    fn chunk_positions(bounds: ScriptSurveyBounds) -> Vec<ChunkPos> {
        let first = ChunkPos {
            x: bounds.min[0].div_euclid(CHUNK_AXIS),
            z: bounds.min[2].div_euclid(CHUNK_AXIS),
        };
        let last = ChunkPos {
            x: bounds.max[0].div_euclid(CHUNK_AXIS),
            z: bounds.max[2].div_euclid(CHUNK_AXIS),
        };
        let mut positions = Vec::new();
        for x in first.x..=last.x {
            for z in first.z..=last.z {
                positions.push(ChunkPos { x, z });
            }
        }
        positions
    }

    fn is_water(&self, state: mc_world::BlockStateId) -> bool {
        self.blocks
            .by_id(state)
            .is_some_and(|state| state.block.id.path() == "water")
    }

    /// Biome string at a world position, if the chunk carries a palette.
    fn biome_at(chunk: &Chunk, x: i32, y: i32, z: i32) -> String {
        let geometry = chunk.geometry();
        let local_y = y - geometry.min_y();
        if local_y < 0 {
            return FALLBACK_BIOME.to_owned();
        }
        let section = (local_y / CHUNK_AXIS) as usize;
        let Some(section) = chunk.biomes.get(section) else {
            return FALLBACK_BIOME.to_owned();
        };
        let cell_y = ((local_y % CHUNK_AXIS) / 4) as u8;
        let cell_x = ((x.rem_euclid(CHUNK_AXIS)) / 4) as u8;
        let cell_z = ((z.rem_euclid(CHUNK_AXIS)) / 4) as u8;
        section.get(cell_x, cell_y, cell_z).to_string()
    }
}

impl SettlementWorld for LiveSettlementWorld {
    fn survey(
        &self,
        plugin_id: &str,
        dimension: &str,
        bounds: ScriptSurveyBounds,
    ) -> Result<SurveyReading, ScriptOperationFailure> {
        if dimension != OVERWORLD {
            // The v1 profile owns overworld terrain only; another dimension is a
            // request core cannot answer rather than an empty survey.
            return Err(ScriptOperationFailure::RuntimeUnavailable);
        }
        let positions = Self::chunk_positions(bounds);
        let snapshot = self.read.snapshot_chunks(&positions);
        let revision = self.next_revision(bounds);
        if positions
            .iter()
            .any(|position| !snapshot.contains_chunk(*position))
        {
            return Ok(SurveyReading {
                usable_plots: 0,
                water_columns: 0,
                claimed: false,
                existing_structures: 0,
                biome_tags: Vec::new(),
                resource_tags: Vec::new(),
                chunk_availability: ScriptChunkAvailability::Unloaded,
                revision,
            });
        }

        let mut biome_tags = BTreeSet::new();
        let mut resource_tags = BTreeSet::new();
        let mut usable_plots = 0_u32;
        let mut water_columns = 0_u32;
        for x in bounds.min[0]..=bounds.max[0] {
            for z in bounds.min[2]..=bounds.max[2] {
                let position = ChunkPos {
                    x: x.div_euclid(CHUNK_AXIS),
                    z: z.div_euclid(CHUNK_AXIS),
                };
                let chunk = snapshot
                    .chunk_ref(position)
                    .expect("survey covered this chunk");
                let local_x = x.rem_euclid(CHUNK_AXIS) as u8;
                let local_z = z.rem_euclid(CHUNK_AXIS) as u8;
                let geometry = chunk.geometry();
                let height = chunk
                    .highest_opaque_y(local_x, local_z)
                    .unwrap_or_else(|| geometry.min_y());

                // Bounded column scan: only the water resting on the surface is
                // measured, never an unbounded volume read.
                let mut water_depth = 0_u8;
                let mut cursor = height + 1;
                let ceiling = geometry
                    .max_y()
                    .min(height + 1 + i32::from(MAX_WATER_DEPTH));
                while cursor < ceiling {
                    let Some(state) = chunk.get_block(local_x, cursor, local_z) else {
                        break;
                    };
                    if !self.is_water(state) {
                        break;
                    }
                    water_depth = water_depth.saturating_add(1);
                    cursor += 1;
                }

                biome_tags.insert(Self::biome_at(chunk, x, height, z));
                if water_depth > 0 {
                    water_columns += 1;
                    resource_tags.insert("water".to_owned());
                } else {
                    usable_plots += 1;
                }
            }
        }

        Ok(SurveyReading {
            usable_plots,
            water_columns,
            claimed: self.claims_overlap(plugin_id, bounds),
            existing_structures: 0,
            biome_tags: biome_tags.into_iter().collect(),
            resource_tags: resource_tags.into_iter().collect(),
            chunk_availability: ScriptChunkAvailability::Loaded,
            revision,
        })
    }

    fn observe_footprint(&self, bounds: ScriptSurveyBounds) -> u64 {
        self.next_revision(bounds)
    }

    fn footprint_changed_since(&self, bounds: ScriptSurveyBounds, revision: u64) -> bool {
        let Ok(observations) = self.observations.lock() else {
            // An unreadable observation ledger cannot prove the footprint is
            // untouched, so the site must be re-surveyed.
            return true;
        };
        let Some(observed) = observations.get(&revision).copied() else {
            return true;
        };
        drop(observations);
        match (observed, self.footprint_digest(bounds)) {
            (Some(before), Some(now)) => before != now,
            // A footprint that could not be read back when it was observed, or
            // that cannot be read back now, cannot be proven unchanged.
            _ => true,
        }
    }

    fn claims_overlap(&self, plugin_id: &str, bounds: ScriptSurveyBounds) -> bool {
        self.zones.as_ref().is_some_and(|zones| {
            zones.foreign_zone_overlaps(plugin_id, OVERWORLD, bounds.min, bounds.max)
        })
    }

    fn max_opaque_y(
        &self,
        bounds: ScriptSurveyBounds,
    ) -> Result<Option<i32>, ScriptOperationFailure> {
        let positions = Self::chunk_positions(bounds);
        let snapshot = self.read.snapshot_chunks(&positions);
        if positions
            .iter()
            .any(|position| !snapshot.contains_chunk(*position))
        {
            // A partly unloaded footprint cannot prove the volume is free.
            return Ok(None);
        }
        let mut highest: Option<i32> = None;
        for x in bounds.min[0]..=bounds.max[0] {
            for z in bounds.min[2]..=bounds.max[2] {
                let position = ChunkPos {
                    x: x.div_euclid(CHUNK_AXIS),
                    z: z.div_euclid(CHUNK_AXIS),
                };
                let chunk = snapshot
                    .chunk_ref(position)
                    .expect("footprint covered this chunk");
                let local_x = x.rem_euclid(CHUNK_AXIS) as u8;
                let local_z = z.rem_euclid(CHUNK_AXIS) as u8;
                let height = chunk
                    .highest_opaque_y(local_x, local_z)
                    .unwrap_or_else(|| chunk.geometry().min_y());
                highest = Some(highest.map_or(height, |current: i32| current.max(height)));
            }
        }
        Ok(highest)
    }

    fn container_reading(
        &self,
        position: [i32; 3],
    ) -> Result<ContainerReading, ScriptOperationFailure> {
        let pos = BlockPos {
            x: position[0],
            y: position[1],
            z: position[2],
        };
        let chunk_pos = ChunkPos {
            x: pos.x.div_euclid(CHUNK_AXIS),
            z: pos.z.div_euclid(CHUNK_AXIS),
        };
        let snapshot = self.read.snapshot_chunks(&[chunk_pos]);
        let Some(chunk) = snapshot.chunk_ref(chunk_pos) else {
            // The chunk is not loaded: a warehouse read must never see this as
            // an empty container.
            return Ok(ContainerReading::Unloaded);
        };
        let Some(chest) = chunk.chests.get(&pos) else {
            // Loaded, but no container block entity sits at the authored
            // position: the position holds something else.
            return Ok(ContainerReading::Missing);
        };
        Ok(ContainerReading::Loaded(
            chest.slots.iter().map(container_slot_to_item).collect(),
        ))
    }

    fn commit_warehouse_transfer(
        &self,
        request: WarehouseTransferRequest,
    ) -> Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + '_>> {
        Box::pin(async move {
            match self.simulation.commit_warehouse_transfer(request).await {
                Ok(WarehouseTransferOutcome::Committed { decision_id }) => Ok(decision_id),
                // A moved container fence and a moved player fence are the one
                // family a transfer fences with: the caller re-reads and retries.
                Ok(
                    WarehouseTransferOutcome::StalePlayer
                    | WarehouseTransferOutcome::StaleContainer,
                ) => Err(ScriptOperationFailure::StaleRevision),
                Ok(WarehouseTransferOutcome::MissingContainer) => {
                    Err(ScriptOperationFailure::NotFound)
                }
                // A closed queue, a refused command or a journal that could not
                // take the decision leaves nothing behind: report the runtime,
                // never a committed deposit.
                Err(_) => Err(ScriptOperationFailure::RuntimeUnavailable),
            }
        })
    }

    fn apply_structure_portion<'a>(
        &'a self,
        _plugin_id: &'a str,
        _structure_id: &'a str,
        blocks: &'a [StructureBlockPlacement],
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptOperationFailure>> + Send + 'a>> {
        Box::pin(async move {
            if blocks.is_empty() || blocks.len() > MAX_WORLD_COMMIT_PORTION {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
            let edits = blocks
                .iter()
                .map(|placement| {
                    BlockEdit::new(
                        BlockPos {
                            x: placement.pos[0],
                            y: placement.pos[1],
                            z: placement.pos[2],
                        },
                        placement.state,
                    )
                })
                .collect::<Vec<_>>();
            // The simulation owns block mutation, so submit the whole portion as
            // exactly one server-owned batch and await the commit. Publication,
            // reactivity and relighting are the pipeline's post-commit projection:
            // a stage is durable only once this command has committed.
            match self.simulation.apply_server_owned_block_edits(edits).await {
                Ok(Some(_outcome)) => Ok(()),
                // A missing, stale or cross-region batch applied nothing; the caller
                // keeps its receipt unspent and retries. An unavailable or closed
                // queue is never reported as a committed portion.
                Ok(None) | Err(_) => Err(ScriptOperationFailure::RuntimeUnavailable),
            }
        })
    }
}

/// Build the observable-settlement identity used by resident generation ids.
///
/// Kept here so the wiring and its tests derive `world_identity` the same way.
pub(crate) fn settlement_world_identity(world_root: &Path) -> String {
    crate::play::persistence::world_identity(world_root)
}
