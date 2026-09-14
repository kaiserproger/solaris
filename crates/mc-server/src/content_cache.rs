//! The single vanilla-content source: discovery and validation.
//!
//! Solaris never redistributes Mojang bytes and never reconstructs their data
//! by guesswork, so every registry, tag, loot table, structure set/template and
//! report the server runs on must come from one cache derived from the
//! operator's own licensed Minecraft artifact. `content_import` is the only
//! writer of that cache; this module only *chooses* and validates it — startup
//! discovers a complete cache, reuses it offline, and otherwise asks the
//! importer to produce one. There is deliberately no embedded-subset startup
//! mode: a silent subset is worse than a loud stop.
//!
//! `crate::startup_data` remains the only consumer of the resolved directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use mc_server::startup_data::validate_vanilla_sidecar_version;

/// Explicit override for the derived content cache root — the one documented
/// escape hatch beside discovery. `[data].vanilla_data_dir` is the file-config
/// spelling of the same thing.
pub const CONTENT_CACHE_ENV: &str = "SOLARIS_CONTENT_CACHE";

/// The command an operator runs to produce a cache by hand.
pub const IMPORT_COMMAND: &str = "mc-server content import --version 26.1.2 --download";

/// User-level cache root: `$XDG_DATA_HOME` (or `~/.local/share`), then
/// `solaris/content/<target release>`.
const USER_CACHE_SUBDIR: &str = "solaris/content";

/// Working-directory cache: the extraction tooling's historical output, still
/// the right place for a source checkout. Gitignored.
const WORKING_CACHE_SUBDIR: &str = "data/vanilla";

/// How many parent directories of the working directory still count as "this
/// checkout" when looking for that cache (`crates/mc-server` from the root, or
/// the root from the crate).
const WORKING_CACHE_ANCESTORS: usize = 4;

/// Cache entries that must exist before the cache may be selected. A partial
/// derivation — datagen without the Java extractors, or without the captured
/// `RegistryData` payloads — is rejected by name instead of being used.
pub const REQUIRED_FILES: &[&str] = &[
    "version.json",
    "data/minecraft/tags",
    "data/minecraft/worldgen/structure_set",
    "data/minecraft/worldgen/structure",
    "data/minecraft/recipe",
    "data/minecraft/loot_table",
    "reports/blocks.json",
    "reports/registries.json",
    "reports/block_light.json",
    "reports/block_mining.json",
    "reports/block_explosion.json",
    "reports/minecraft/components/item",
];

/// The derived payload directory carrying the exact `RegistryData` bytes a real
/// client login needs (`VanillaData::has_full_registry_payloads`).
pub const REQUIRED_REGISTRY_PAYLOAD_DIR: &str = "reports/registry_network_nbt";

/// A complete, validated vanilla-content cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentCache {
    root: PathBuf,
}

impl ContentCache {
    /// Validate an already-chosen cache root.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        validate_content_cache(&root)?;
        Ok(Self { root })
    }

    /// Wrap a root the importer has already validated.
    pub(crate) fn from_validated(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Every location discovery tries, in order, so a failure can name them all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentSearch {
    /// `[data].vanilla_data_dir`, when configured.
    pub explicit: Option<PathBuf>,
    /// `SOLARIS_CONTENT_CACHE`, when set.
    pub environment: Option<PathBuf>,
    /// Standard locations, in priority order.
    pub standard: Vec<PathBuf>,
}

impl ContentSearch {
    /// Build the search set from configuration and environment.
    ///
    /// `home` and `working_dir` are injected so the result is testable without
    /// touching the real environment.
    #[must_use]
    pub fn from_config(
        explicit: Option<&Path>,
        environment: Option<PathBuf>,
        home: Option<&Path>,
        working_dir: &Path,
    ) -> Self {
        let mut standard = Vec::new();
        if let Some(home) = home {
            let data_home = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local").join("share"));
            standard.push(
                data_home
                    .join(USER_CACHE_SUBDIR)
                    .join(mc_protocol::TARGET_RELEASE),
            );
        }
        // The working directory and its ancestors: a cache populated in a
        // source checkout must be found both from the repo root and from a
        // crate subdirectory (where `cargo test` puts the process).
        let mut directory = Some(working_dir);
        for _ in 0..=WORKING_CACHE_ANCESTORS {
            let Some(current) = directory else { break };
            let candidate = current.join(WORKING_CACHE_SUBDIR);
            if !standard.contains(&candidate) {
                standard.push(candidate);
            }
            directory = current.parent();
        }
        Self {
            explicit: explicit.map(Path::to_path_buf),
            environment,
            standard,
        }
    }

    /// Every candidate root, in the order discovery tries them.
    #[must_use]
    pub fn candidates(&self) -> Vec<PathBuf> {
        let mut all = Vec::new();
        if let Some(explicit) = &self.explicit {
            all.push(explicit.clone());
        }
        if let Some(environment) = &self.environment {
            all.push(environment.clone());
        }
        all.extend(self.standard.iter().cloned());
        all
    }

    /// Where an automatic import should publish: the configured override when
    /// there is one, otherwise the highest-priority standard location.
    #[must_use]
    pub fn import_target(&self) -> PathBuf {
        if let Some(explicit) = &self.explicit {
            return explicit.clone();
        }
        if let Some(environment) = &self.environment {
            return environment.clone();
        }
        self.standard
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from(WORKING_CACHE_SUBDIR))
    }
}

/// Resolve the one content cache the server runs on.
///
/// The explicit override and `SOLARIS_CONTENT_CACHE` are authoritative: when
/// set, an invalid cache is an error rather than a silent fall-through to
/// another location, which would quietly change content. Standard locations are
/// tried in order and the first *complete* cache wins. When nothing is usable
/// the error names the missing prerequisite, every searched location, and the
/// command that produces a cache.
pub fn discover_content_cache(search: &ContentSearch) -> Result<ContentCache> {
    for (index, candidate) in search.candidates().iter().enumerate() {
        let authoritative = match (search.explicit.as_ref(), search.environment.as_ref()) {
            (Some(_), _) => index == 0,
            (None, Some(_)) => index == 0,
            (None, None) => false,
        };
        if authoritative {
            return ContentCache::open(candidate.clone()).with_context(|| {
                format!(
                    "the configured vanilla content cache at {} is not usable",
                    candidate.display()
                )
            });
        }
        if validate_content_cache(candidate).is_ok() {
            return Ok(ContentCache::from_validated(candidate.clone()));
        }
    }

    let searched = search
        .candidates()
        .iter()
        .map(|path| format!("  {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    bail!(
        "no vanilla content cache found. Solaris runs on data derived from your own \
         licensed Minecraft Java {release} installation and never redistributes or \
         guesses it, so a complete cache is required before startup.\n\
         searched:\n{searched}\n\
         produce one with `{IMPORT_COMMAND}` (a JDK {java} is required by that command), \
         or set [data].vanilla_data_dir to an already-derived cache root",
        release = mc_protocol::TARGET_RELEASE,
        java = 25,
    );
}

/// Validate that `root` is a complete derived cache for the target release.
pub fn validate_content_cache(root: &Path) -> Result<()> {
    validate_vanilla_sidecar_version(root)
        .with_context(|| format!("validating vanilla content cache at {}", root.display()))?;
    for relative in REQUIRED_FILES {
        let path = root.join(relative);
        if !path.exists() {
            bail!(
                "vanilla content cache at {} is incomplete: missing {relative}; \
                 produce a complete cache with `{IMPORT_COMMAND}`",
                root.display()
            );
        }
    }
    if !root.join(REQUIRED_REGISTRY_PAYLOAD_DIR).is_dir() {
        bail!(
            "vanilla content cache at {} is incomplete: missing {REQUIRED_REGISTRY_PAYLOAD_DIR}, \
             the exact RegistryData payloads a client login requires; produce a complete cache \
             with `{IMPORT_COMMAND}`",
            root.display()
        );
    }
    Ok(())
}
