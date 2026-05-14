//! # mc-data
//!
//! Read-only index into the vanilla data sidecar described in
//! [`docs/decisions/0001-vanilla-data-as-runtime-input.md`]. M1.f-scope:
//! enumerate the entries in each registry by scanning for JSON files in
//! `<data_dir>/data/<namespace>/<registry>/**/*.json`. The actual
//! contents are not parsed — M1.g sends `Registry Data` with
//! `has_data = false` for every entry and lets the client read its own
//! built-in pack via Known Packs.
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

pub mod biomes;
pub mod block_light;
pub mod blocks;
pub mod entity_types;
pub mod identifier;
pub mod items;
pub mod tags;
pub mod worldgen_ores;

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
    ("enchantment_provider", "enchantment_provider"),
    ("frog_variant", "frog_variant"),
    ("instrument", "instrument"),
    ("jukebox_song", "jukebox_song"),
    ("painting_variant", "painting_variant"),
    ("pig_sound_variant", "pig_sound_variant"),
    ("pig_variant", "pig_variant"),
    ("timeline", "timeline"),
    ("trade_set", "trade_set"),
    ("trim_material", "trim_material"),
    ("trim_pattern", "trim_pattern"),
    ("villager_trade", "villager_trade"),
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
}

impl VanillaData {
    /// Path the registries were loaded from. Empty for in-memory
    /// constructions (e.g. test stubs).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
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
        collect_entries(&dir, &dir, &mut entries)?;
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

    Ok(VanillaData { root, registries })
}

fn collect_entries(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), DataError> {
    let entries = std::fs::read_dir(dir).map_err(|source| DataError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| DataError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let ty = entry.file_type().map_err(|source| DataError::Io {
            path: path.clone(),
            source,
        })?;
        if ty.is_dir() {
            collect_entries(root, &path, out)?;
        } else if ty.is_file() && path.extension().is_some_and(|e| e == "json") {
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
        }
    }
    Ok(())
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
    }
}
