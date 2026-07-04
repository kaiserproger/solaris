//! # mc-data
//!
//! Read-only indexes into the vanilla data sidecar described in
//! [`docs/decisions/0001-vanilla-data-as-runtime-input.md`]. Registry
//! entries are discovered from `<data_dir>/data/<namespace>/<registry>/**/*.json`;
//! selected reports, tags, recipes, item facts, block facts, and worldgen
//! facts are parsed by focused loaders.
//!
//! Concrete file layout (per registry):
//!
//! ```text
//! <data_dir>/data/<namespace>/<registry-path>/<sub>/.../<name>.json
//!                       ^ namespace, e.g. "minecraft"
//!                                   ^ registry, e.g. "dimension_type" or
//!                                                    "worldgen/biome"
//! ```
//!
//! The registry's *path* may be nested (e.g. `worldgen/biome`); the
//! entries inside may also be nested (e.g. recipes use subfolders).
//! Both are flattened: an entry's `Identifier` path is the part *after*
//! the registry root, with `/` separators and no `.json` suffix —
//! exactly what vanilla uses on the wire.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, trace};

const REQUIRED_REGISTRY_INDEX: &str = include_str!("../data/required_registry_index.json");

pub mod armor;
pub mod biomes;
pub mod block_entity_types;
pub mod block_facts;
pub mod block_light;
pub mod blocks;
pub mod damage_types;
pub mod entity_types;
pub mod food;
pub mod identifier;
pub mod item_components;
pub mod items;
pub mod loot;
pub mod recipes;
pub mod tags;
pub mod worldgen_features;
pub mod worldgen_inventory;
pub mod worldgen_ores;
pub mod worldgen_structures;

pub use identifier::{Identifier, IdentifierError};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// All vanilla registries Solaris needs to surface in the Configuration
/// state. Keep in sync with `tools/extract-vanilla-data.sh`.
///
/// `(registry_id_path, fs_subpath_under_data_minecraft)` — the two are
/// identical for everything except `worldgen/biome`, which protocol
/// names as `minecraft:worldgen/biome` and stores under `worldgen/biome/`.
pub const KNOWN_REGISTRIES: &[(&str, &str)] = &[
    ("banner_pattern", "banner_pattern"),
    ("cat_sound_variant", "cat_sound_variant"),
    ("cat_variant", "cat_variant"),
    ("chat_type", "chat_type"),
    ("chicken_sound_variant", "chicken_sound_variant"),
    ("chicken_variant", "chicken_variant"),
    ("cow_sound_variant", "cow_sound_variant"),
    ("cow_variant", "cow_variant"),
    ("damage_type", "damage_type"),
    ("dialog", "dialog"),
    ("dimension_type", "dimension_type"),
    ("enchantment", "enchantment"),
    ("frog_variant", "frog_variant"),
    ("instrument", "instrument"),
    ("jukebox_song", "jukebox_song"),
    ("painting_variant", "painting_variant"),
    ("pig_sound_variant", "pig_sound_variant"),
    ("pig_variant", "pig_variant"),
    ("test_environment", "test_environment"),
    ("test_instance", "test_instance"),
    ("timeline", "timeline"),
    ("trim_material", "trim_material"),
    ("trim_pattern", "trim_pattern"),
    ("wolf_sound_variant", "wolf_sound_variant"),
    ("wolf_variant", "wolf_variant"),
    ("world_clock", "world_clock"),
    ("worldgen/biome", "worldgen/biome"),
    ("zombie_nautilus_variant", "zombie_nautilus_variant"),
];

#[derive(Debug, Error)]
pub enum DataError {
    #[error(
        "vanilla data directory does not exist at {0}; \
         did you run tools/extract-vanilla-data.sh?"
    )]
    Missing(PathBuf),

    #[error("required registry {registry} is empty or missing at {path}")]
    EmptyRegistry { registry: String, path: PathBuf },

    #[error("entry id {entry:?} (from {path}) is not a valid identifier path")]
    InvalidEntry { entry: String, path: PathBuf },

    #[error("filesystem error walking {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// One built-in registry — the registry name plus the sorted list of
/// entry identifiers found on disk.
#[derive(Debug, Clone)]
pub struct Registry {
    /// `minecraft:dimension_type`, `minecraft:worldgen/biome`, …
    pub id: Identifier,
    /// Entry identifiers in lexicographic order. `minecraft:overworld`,
    /// `minecraft:plains`, etc. Vanilla loads in registration order,
    /// which differs; per ADR 0001 we ship our own ordering and use it
    /// consistently in every packet we emit.
    pub entries: Vec<Identifier>,
}

/// The whole vanilla-data sidecar, indexed.
#[derive(Debug, Clone)]
pub struct VanillaData {
    root: PathBuf,
    registries: BTreeMap<String, Registry>,
    sidecar_root: bool,
}

impl VanillaData {
    /// Path the registries were loaded from. Empty for in-memory
    /// constructions (e.g. test stubs).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Filesystem sidecar root when this index came from [`load`]. Embedded or
    /// in-memory test registries are not sidecar data even if their diagnostic
    /// root string happens to name an existing path.
    #[must_use]
    pub fn sidecar_root(&self) -> Option<&Path> {
        self.sidecar_root.then_some(self.root.as_path())
    }

    /// Iterate every loaded registry in registry-id order.
    pub fn registries(&self) -> impl Iterator<Item = &Registry> {
        self.registries.values()
    }

    /// Look up a registry by its id (`"minecraft:dimension_type"`,
    /// without the namespace prefix, or with — we accept both).
    #[must_use]
    pub fn registry(&self, name: &str) -> Option<&Registry> {
        let key = name.strip_prefix("minecraft:").unwrap_or(name);
        self.registries.get(key)
    }

    /// Number of indexed registries.
    #[must_use]
    pub fn registry_count(&self) -> usize {
        self.registries.len()
    }

    /// Total number of entries across all registries.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.registries.values().map(|r| r.entries.len()).sum()
    }

    /// Build a `VanillaData` from in-memory registries. Used by tests
    /// that don't want to stage a filesystem layout and by future code
    /// that loads from somewhere other than disk. `root` is recorded
    /// only for diagnostic purposes.
    #[must_use]
    pub fn from_registries(root: impl Into<PathBuf>, registries: Vec<Registry>) -> Self {
        let map = registries
            .into_iter()
            .map(|r| (r.id.path().to_string(), r))
            .collect();
        Self {
            root: root.into(),
            registries: map,
            sidecar_root: false,
        }
    }
}

/// Helpers used only from tests. Public so integration tests in other
/// crates can build a `VanillaData` without staging a filesystem layout.
#[doc(hidden)]
pub mod testing {
    use super::{Identifier, KNOWN_REGISTRIES, Registry, VanillaData};

    /// A minimal `VanillaData` with every known registry populated with
    /// two placeholder entries. Useful as a stub for tests that exercise
    /// the network handler without caring about specific entry contents.
    #[must_use]
    pub fn stub() -> VanillaData {
        let registries = KNOWN_REGISTRIES
            .iter()
            .map(|(path, _)| {
                let id = Identifier::parse(format!("minecraft:{path}"))
                    .expect("KNOWN_REGISTRIES paths are valid identifiers");
                let entries = vec![
                    Identifier::parse("minecraft:alpha").unwrap(),
                    Identifier::parse("minecraft:beta").unwrap(),
                ];
                Registry { id, entries }
            })
            .collect();
        VanillaData::from_registries("", registries)
    }
}

/// Repo-owned registry index used when the local vanilla sidecar is absent.
/// Entries are the minimal identifiers Solaris needs to complete the
/// Configuration state and keep worldgen/survival baselines running.
#[must_use]
pub fn solaris_required_data() -> VanillaData {
    let index: BTreeMap<String, Vec<String>> = serde_json::from_str(REQUIRED_REGISTRY_INDEX)
        .expect("embedded required registry index JSON is valid");

    let registries = KNOWN_REGISTRIES
        .iter()
        .map(|(path, _)| {
            let entries = index
                .get(*path)
                .unwrap_or_else(|| panic!("embedded required registry index missing {path}"))
                .iter()
                .map(|entry| {
                    Identifier::parse(entry.clone())
                        .expect("embedded required registry entry id is valid")
                })
                .collect();
            Registry {
                id: Identifier::parse(format!("minecraft:{path}"))
                    .expect("KNOWN_REGISTRIES paths are valid identifiers"),
                entries,
            }
        })
        .collect();
    VanillaData::from_registries("<solaris-built-in>", registries)
}

/// Load the vanilla data sidecar rooted at `path` (typically
/// `data/vanilla/`).
///
/// Walks `<path>/data/minecraft/<registry>/**/*.json` for each
/// well-known registry. Registries that have zero entries are an error,
/// because that means either the operator forgot to run the extraction
/// script or this Mojang version dropped a registry we still expect.
pub fn load(path: impl Into<PathBuf>) -> Result<VanillaData, DataError> {
    let root = path.into();
    if !root.is_dir() {
        return Err(DataError::Missing(root));
    }
    let mc_root = root.join("data").join("minecraft");

    let mut registries = BTreeMap::new();
    for (registry_path, fs_subpath) in KNOWN_REGISTRIES {
        let dir = mc_root.join(fs_subpath);
        if !dir.is_dir() {
            return Err(DataError::EmptyRegistry {
                registry: (*registry_path).to_string(),
                path: dir,
            });
        }
        let mut entries = Vec::new();
        collect_entries(&dir, &mut entries)?;
        if entries.is_empty() {
            return Err(DataError::EmptyRegistry {
                registry: (*registry_path).to_string(),
                path: dir,
            });
        }
        entries.sort();

        let identifiers = entries
            .into_iter()
            .map(|stem| {
                let qualified = format!("minecraft:{stem}");
                Identifier::parse(qualified.clone()).map_err(|_| DataError::InvalidEntry {
                    entry: qualified,
                    path: dir.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let registry_id = Identifier::parse(format!("minecraft:{registry_path}"))
            .expect("KNOWN_REGISTRIES paths are well-formed");
        debug!(
            registry = %registry_id,
            entries = identifiers.len(),
            "loaded registry"
        );
        registries.insert(
            (*registry_path).to_string(),
            Registry {
                id: registry_id,
                entries: identifiers,
            },
        );
    }

    Ok(VanillaData {
        root,
        registries,
        sidecar_root: true,
    })
}

pub(crate) fn visit_json_files<E>(
    dir: &Path,
    visitor: &mut impl FnMut(PathBuf) -> Result<(), E>,
    io_error: &impl Fn(PathBuf, std::io::Error) -> E,
) -> Result<(), E> {
    let entries = std::fs::read_dir(dir).map_err(|source| io_error(dir.to_path_buf(), source))?;
    for entry in entries {
        let entry = entry.map_err(|source| io_error(dir.to_path_buf(), source))?;
        let path = entry.path();
        let ty = entry
            .file_type()
            .map_err(|source| io_error(path.clone(), source))?;
        if ty.is_dir() {
            visit_json_files(&path, visitor, io_error)?;
        } else if ty.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            visitor(path)?;
        }
    }
    Ok(())
}

pub(crate) fn sorted_json_files<E>(
    dir: &Path,
    io_error: &impl Fn(PathBuf, std::io::Error) -> E,
) -> Result<Vec<PathBuf>, E> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|source| io_error(dir.to_path_buf(), source))? {
        let entry = entry.map_err(|source| io_error(dir.to_path_buf(), source))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub(crate) fn read_json_file<T, E>(
    path: &Path,
    io_error: &impl Fn(PathBuf, std::io::Error) -> E,
    parse_error: &impl Fn(PathBuf, serde_json::Error) -> E,
) -> Result<T, E>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let bytes = std::fs::read(path).map_err(|source| io_error(path.to_path_buf(), source))?;
    serde_json::from_slice(&bytes).map_err(|source| parse_error(path.to_path_buf(), source))
}

fn collect_entries(root: &Path, out: &mut Vec<String>) -> Result<(), DataError> {
    visit_json_files(
        root,
        &mut |path| {
            let rel = path
                .strip_prefix(root)
                .expect("recursive walk yields paths under root");
            let stem = rel.with_extension("");
            let mut joined = String::new();
            for component in stem.components() {
                if !joined.is_empty() {
                    joined.push('/');
                }
                match component.as_os_str().to_str() {
                    Some(s) => joined.push_str(s),
                    None => {
                        return Err(DataError::InvalidEntry {
                            entry: stem.to_string_lossy().into_owned(),
                            path: path.clone(),
                        });
                    }
                }
            }
            trace!(file = %path.display(), entry = %joined, "indexed");
            out.push(joined);
            Ok(())
        },
        &|path, source| DataError::Io { path, source },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_minimal_layout() -> TempDir {
        let dir = TempDir::new().unwrap();
        let mc = dir.path().join("data").join("minecraft");
        for (registry_path, _) in KNOWN_REGISTRIES {
            let registry_dir = mc.join(registry_path);
            fs::create_dir_all(&registry_dir).unwrap();
            fs::write(registry_dir.join("alpha.json"), "{}").unwrap();
            fs::write(registry_dir.join("beta.json"), "{}").unwrap();
        }
        dir
    }

    #[test]
    fn load_indexes_every_known_registry() {
        let dir = make_minimal_layout();
        let data = load(dir.path()).unwrap();
        assert_eq!(data.sidecar_root(), Some(dir.path()));
        assert_eq!(data.registry_count(), KNOWN_REGISTRIES.len());
        for (registry_path, _) in KNOWN_REGISTRIES {
            let reg = data
                .registry(registry_path)
                .unwrap_or_else(|| panic!("missing registry {registry_path}"));
            assert_eq!(reg.entries.len(), 2);
            assert_eq!(
                reg.entries[0].as_str(),
                "minecraft:alpha",
                "entries should be lexicographically sorted"
            );
        }
    }

    #[test]
    fn registry_lookup_accepts_namespaced_id() {
        let dir = make_minimal_layout();
        let data = load(dir.path()).unwrap();
        assert!(data.registry("minecraft:dimension_type").is_some());
        assert!(data.registry("dimension_type").is_some());
        assert!(data.registry("nonexistent").is_none());
    }

    #[test]
    fn embedded_required_registry_index_covers_known_registries() {
        let data = solaris_required_data();

        assert!(data.sidecar_root().is_none());
        assert_eq!(data.registry_count(), KNOWN_REGISTRIES.len());
        assert!(data.entry_count() > 300);
        assert!(
            data.registry("minecraft:worldgen/biome")
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.as_str() == "minecraft:plains")
        );
        assert!(
            data.registry("minecraft:dimension_type")
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.as_str() == "minecraft:overworld")
        );
        assert_eq!(
            data.registry("minecraft:test_environment")
                .unwrap()
                .entries
                .iter()
                .map(Identifier::as_str)
                .collect::<Vec<_>>(),
            vec!["minecraft:default"]
        );
        assert_eq!(
            data.registry("minecraft:test_instance")
                .unwrap()
                .entries
                .iter()
                .map(Identifier::as_str)
                .collect::<Vec<_>>(),
            vec!["minecraft:always_pass"]
        );
        assert!(data.registry("minecraft:enchantment_provider").is_none());
        assert!(data.registry("minecraft:trade_set").is_none());
        assert!(data.registry("minecraft:villager_trade").is_none());
    }

    #[test]
    fn missing_root_is_reported_clearly() {
        let err = load(PathBuf::from("/definitely/does/not/exist")).unwrap_err();
        assert!(matches!(err, DataError::Missing(_)));
    }

    #[test]
    fn empty_registry_is_an_error() {
        let dir = TempDir::new().unwrap();
        let mc = dir.path().join("data").join("minecraft");
        for (registry_path, _) in KNOWN_REGISTRIES {
            let registry_dir = mc.join(registry_path);
            fs::create_dir_all(&registry_dir).unwrap();
            if registry_path != &"dimension_type" {
                fs::write(registry_dir.join("alpha.json"), "{}").unwrap();
            }
        }
        let err = load(dir.path()).unwrap_err();
        assert!(matches!(
            err,
            DataError::EmptyRegistry { ref registry, .. } if registry == "dimension_type"
        ));
    }

    #[test]
    fn nested_entries_use_forward_slashes() {
        let dir = TempDir::new().unwrap();
        let mc = dir.path().join("data").join("minecraft");
        for (registry_path, _) in KNOWN_REGISTRIES {
            let registry_dir = mc.join(registry_path);
            fs::create_dir_all(&registry_dir).unwrap();
            fs::write(registry_dir.join("alpha.json"), "{}").unwrap();
        }
        let biome = mc.join("worldgen").join("biome");
        fs::create_dir_all(biome.join("nether")).unwrap();
        fs::write(biome.join("nether").join("warped_forest.json"), "{}").unwrap();

        let data = load(dir.path()).unwrap();
        let reg = data.registry("worldgen/biome").unwrap();
        let nested = reg
            .entries
            .iter()
            .find(|e| e.path() == "nether/warped_forest")
            .expect("nested entry indexed");
        assert_eq!(nested.as_str(), "minecraft:nether/warped_forest");
    }

    /// If a real vanilla extraction is present locally, sanity-check the
    /// loader against it. Skipped otherwise so CI without
    /// `.analysis/server.jar` still passes.
    #[test]
    fn loads_real_vanilla_sidecar_when_present() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("data")
            .join("vanilla");
        if !path.join("data").join("minecraft").is_dir() {
            eprintln!("skipping: {} not populated", path.display());
            return;
        }
        let data = load(&path).expect("real vanilla data should load");
        assert!(data.registry_count() >= KNOWN_REGISTRIES.len());
        let dim = data.registry("dimension_type").unwrap();
        assert!(
            dim.entries
                .iter()
                .any(|id| id.as_str() == "minecraft:overworld"),
            "vanilla dimension_type should always contain overworld; got {:?}",
            dim.entries
        );
        let biomes = data.registry("worldgen/biome").unwrap();
        assert!(biomes.entries.len() >= 30, "expected ≥ 30 vanilla biomes");
        assert!(data.registry("test_environment").is_some());
        assert!(data.registry("test_instance").is_some());
        assert!(data.registry("enchantment_provider").is_none());
        assert!(data.registry("trade_set").is_none());
        assert!(data.registry("villager_trade").is_none());
    }
}
