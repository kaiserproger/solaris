//! Tag-network data for the `Update Tags` packet.
//!
//! Walks `<vanilla_dir>/data/minecraft/tags/<root>/**/*.json`, resolves
//! `#tag`-references transitively, dedupes entries, and maps each
//! entry identifier to the numeric wire id the client expects:
//!
//! - For *data-driven* registries the server itself sends via
//!   `RegistryData` (enchantment, damage_type, instrument,
//!   banner_pattern, painting_variant, worldgen/biome) — the entry id
//!   is the position of the entry in our sorted `Registry.entries`.
//!   The client indexes by send order, not by Mojang's `protocol_id`.
//! - For *built-in* registries the client knows natively (item, block,
//!   entity_type, fluid, game_event, …) — the entry id is the
//!   `protocol_id` from `<vanilla_dir>/reports/registries.json`.
//!
//! Output is a flat `TagsData` ready to feed into one
//! `mc_protocol::packets::configuration::UpdateTags` packet.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, warn};

use crate::{Identifier, VanillaData};

/// `(tags/<fs subpath>, "minecraft:<registry id>")` pairs the loader
/// knows about. We deliberately limit the set to registries the
/// vanilla client either knows natively (built-in `block`, `item`,
/// `entity_type`, `fluid`, `game_event`, `point_of_interest_type`,
/// `potion`) or that *we* send via `RegistryData` (anything in
/// [`KNOWN_REGISTRIES`]). Sending tags for a registry the client has
/// never heard of trips
/// `RegistryAccess.lookupOrThrow → Missing registry: …` during the
/// Configuration → Play transition, even if the tag entry list is
/// empty — see the M3.i log for the wire repro.
///
/// Registries excluded on purpose: `dialog`, `timeline`,
/// `villager_trade`, `worldgen/configured_feature`,
/// `worldgen/flat_level_generator_preset`, `worldgen/structure`,
/// `worldgen/world_preset`. We do not ship their entries via
/// `RegistryData` (they are data-driven but not in
/// [`KNOWN_REGISTRIES`]) and the client doesn't have them built in.
const TAG_ROOTS: &[(&str, &str)] = &[
    ("banner_pattern", "minecraft:banner_pattern"),
    ("block", "minecraft:block"),
    ("damage_type", "minecraft:damage_type"),
    ("dialog", "minecraft:dialog"),
    ("enchantment", "minecraft:enchantment"),
    ("entity_type", "minecraft:entity_type"),
    ("fluid", "minecraft:fluid"),
    ("game_event", "minecraft:game_event"),
    ("instrument", "minecraft:instrument"),
    ("item", "minecraft:item"),
    ("painting_variant", "minecraft:painting_variant"),
    ("point_of_interest_type", "minecraft:point_of_interest_type"),
    ("potion", "minecraft:potion"),
    ("timeline", "minecraft:timeline"),
    ("worldgen/biome", "minecraft:worldgen/biome"),
];

#[derive(Debug, Error)]
pub enum TagError {
    #[error("registries.json not found at {0}; run tools/extract-vanilla-data.sh --reports")]
    RegistriesMissing(PathBuf),
    #[error("registries.json at {path} is malformed: {source}")]
    RegistriesMalformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("filesystem error walking {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("tag file {path} malformed: {source}")]
    TagMalformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Resolved tags, grouped by registry id, ready for the
/// `Update Tags` packet's `Map<registry, Map<tag, int[]>>` payload.
#[derive(Debug, Clone, Default)]
pub struct TagsData {
    pub registries: BTreeMap<Identifier, BTreeMap<Identifier, Vec<i32>>>,
}

impl TagsData {
    /// Number of `(registry, tag)` pairs the packet will emit.
    #[must_use]
    pub fn total_tags(&self) -> usize {
        self.registries.values().map(|m| m.len()).sum()
    }

    /// Total number of `(registry, tag, entry)` triples — handy for
    /// startup logging.
    #[must_use]
    pub fn total_entries(&self) -> usize {
        self.registries
            .values()
            .flat_map(|m| m.values())
            .map(Vec::len)
            .sum()
    }
}

#[derive(Deserialize)]
struct RawTag {
    #[serde(default)]
    #[allow(dead_code)]
    replace: bool,
    #[serde(default)]
    values: Vec<RawTagValue>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawTagValue {
    Plain(String),
    Object {
        id: String,
        #[serde(default = "default_required")]
        required: bool,
    },
}

fn default_required() -> bool {
    true
}

#[derive(Deserialize)]
struct ProtocolIdEntry {
    protocol_id: i32,
}

#[derive(Deserialize)]
struct RegistryReport {
    #[serde(default)]
    entries: BTreeMap<String, ProtocolIdEntry>,
}

/// Numeric id of `entry_id` inside `registry_id`, preferring the
/// position our `RegistryData` will use over the vanilla `protocol_id`
/// for data-driven registries.
fn entry_id_for(
    registry_id: &str,
    entry_id: &str,
    ours: &VanillaData,
    vanilla_ids: &BTreeMap<String, BTreeMap<String, i32>>,
) -> Option<i32> {
    let path = registry_id
        .strip_prefix("minecraft:")
        .unwrap_or(registry_id);
    if let Some(reg) = ours.registry(path)
        && let Some(pos) = reg.entries.iter().position(|e| e.as_str() == entry_id)
    {
        return Some(pos as i32);
    }
    vanilla_ids.get(registry_id)?.get(entry_id).copied()
}

fn load_vanilla_id_index(
    vanilla_dir: &Path,
) -> Result<BTreeMap<String, BTreeMap<String, i32>>, TagError> {
    let path = vanilla_dir.join("reports").join("registries.json");
    if !path.is_file() {
        return Err(TagError::RegistriesMissing(path));
    }
    let raw = std::fs::read_to_string(&path).map_err(|source| TagError::Io {
        path: path.clone(),
        source,
    })?;
    let report: BTreeMap<String, RegistryReport> =
        serde_json::from_str(&raw).map_err(|source| TagError::RegistriesMalformed {
            path: path.clone(),
            source,
        })?;
    Ok(report
        .into_iter()
        .map(|(reg, body)| {
            let entries = body
                .entries
                .into_iter()
                .map(|(id, v)| (id, v.protocol_id))
                .collect();
            (reg, entries)
        })
        .collect())
}

fn collect_tag_files(
    root: &Path,
    dir: &Path,
    registry_id: &str,
    raw: &mut BTreeMap<(String, String), (PathBuf, RawTag)>,
) -> Result<(), TagError> {
    let entries = std::fs::read_dir(dir).map_err(|source| TagError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| TagError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let ty = entry.file_type().map_err(|source| TagError::Io {
            path: path.clone(),
            source,
        })?;
        if ty.is_dir() {
            collect_tag_files(root, &path, registry_id, raw)?;
        } else if ty.is_file() && path.extension().is_some_and(|e| e == "json") {
            let rel = path
                .strip_prefix(root)
                .expect("walk yields paths under root")
                .with_extension("");
            let mut joined = String::new();
            for component in rel.components() {
                if !joined.is_empty() {
                    joined.push('/');
                }
                joined.push_str(component.as_os_str().to_string_lossy().as_ref());
            }
            let body = std::fs::read_to_string(&path).map_err(|source| TagError::Io {
                path: path.clone(),
                source,
            })?;
            let parsed: RawTag =
                serde_json::from_str(&body).map_err(|source| TagError::TagMalformed {
                    path: path.clone(),
                    source,
                })?;
            raw.insert((registry_id.to_string(), joined), (path, parsed));
        }
    }
    Ok(())
}

fn resolve(
    registry_id: &str,
    tag_path: &str,
    raw: &BTreeMap<(String, String), (PathBuf, RawTag)>,
    ours: &VanillaData,
    vanilla_ids: &BTreeMap<String, BTreeMap<String, i32>>,
    visiting: &mut BTreeSet<String>,
    seen: &mut BTreeSet<i32>,
) {
    let marker = format!("{registry_id}#{tag_path}");
    if visiting.contains(&marker) {
        // Cycle — drop the back-edge silently. Vanilla does the same.
        return;
    }
    let Some((_, raw_tag)) = raw.get(&(registry_id.to_string(), tag_path.to_string())) else {
        // Dangling `#tag` reference — vanilla treats as empty.
        return;
    };
    visiting.insert(marker.clone());
    for v in &raw_tag.values {
        let (raw_value, required) = match v {
            RawTagValue::Plain(s) => (s.as_str(), true),
            RawTagValue::Object { id, required } => (id.as_str(), *required),
        };
        if let Some(tag_ref) = raw_value.strip_prefix('#') {
            let inner_path = tag_ref
                .strip_prefix("minecraft:")
                .unwrap_or_else(|| tag_ref.split_once(':').map_or(tag_ref, |(_, p)| p));
            resolve(
                registry_id,
                inner_path,
                raw,
                ours,
                vanilla_ids,
                visiting,
                seen,
            );
        } else if let Some(idx) = entry_id_for(registry_id, raw_value, ours, vanilla_ids) {
            seen.insert(idx);
        } else if required {
            warn!(
                registry = %registry_id,
                tag = %tag_path,
                value = %raw_value,
                "tag references unknown registry entry; skipping"
            );
        }
    }
    visiting.remove(&marker);
}

/// Load the full tag set for `vanilla_dir`. Returns an empty `TagsData`
/// when no tag-root directories exist (the sidecar was generated with
/// `--reports` only but no `tags/` directory).
pub fn load(vanilla_dir: &Path, ours: &VanillaData) -> Result<TagsData, TagError> {
    let vanilla_ids = load_vanilla_id_index(vanilla_dir)?;
    let tags_root = vanilla_dir.join("data").join("minecraft").join("tags");

    let mut raw: BTreeMap<(String, String), (PathBuf, RawTag)> = BTreeMap::new();
    for (subpath, registry_id) in TAG_ROOTS {
        let root = tags_root.join(subpath);
        if !root.is_dir() {
            continue;
        }
        collect_tag_files(&root, &root, registry_id, &mut raw)?;
    }

    let mut registries: BTreeMap<Identifier, BTreeMap<Identifier, Vec<i32>>> = BTreeMap::new();
    for (registry_id, tag_path) in raw.keys() {
        let mut visiting = BTreeSet::new();
        let mut seen = BTreeSet::new();
        resolve(
            registry_id,
            tag_path,
            &raw,
            ours,
            &vanilla_ids,
            &mut visiting,
            &mut seen,
        );

        let registry_ident =
            Identifier::parse(registry_id.clone()).expect("TAG_ROOTS provides valid identifiers");
        let tag_ident = Identifier::parse(format!("minecraft:{tag_path}"))
            .expect("tag path is a valid identifier");
        registries
            .entry(registry_ident)
            .or_default()
            .insert(tag_ident, seen.into_iter().collect());
    }

    let data = TagsData { registries };
    debug!(
        registries = data.registries.len(),
        tags = data.total_tags(),
        entries = data.total_entries(),
        "loaded vanilla tag set"
    );
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_tiny_sidecar() -> TempDir {
        // Minimal vanilla sidecar: registries.json with one item, one
        // tags file that references it, and a `#tag`-ref to a sibling.
        let dir = TempDir::new().unwrap();
        let reports = dir.path().join("reports");
        fs::create_dir_all(&reports).unwrap();
        fs::write(
            reports.join("registries.json"),
            r#"{
                "minecraft:item": {
                    "entries": {
                        "minecraft:apple": { "protocol_id": 5 },
                        "minecraft:carrot": { "protocol_id": 7 }
                    }
                }
            }"#,
        )
        .unwrap();
        let tags_item = dir
            .path()
            .join("data")
            .join("minecraft")
            .join("tags")
            .join("item");
        fs::create_dir_all(tags_item.join("food")).unwrap();
        fs::write(
            tags_item.join("food").join("snacks.json"),
            r##"{ "values": [ "minecraft:apple" ] }"##,
        )
        .unwrap();
        fs::write(
            tags_item.join("everything.json"),
            r##"{ "values": [ "#minecraft:food/snacks", "minecraft:carrot" ] }"##,
        )
        .unwrap();
        // Tag that points at a missing entry to make sure we keep going.
        fs::write(
            tags_item.join("hopeful.json"),
            r##"{ "values": [ { "id": "minecraft:nope", "required": false } ] }"##,
        )
        .unwrap();
        dir
    }

    fn empty_vanilla_data() -> VanillaData {
        VanillaData::from_registries("", vec![])
    }

    #[test]
    fn resolves_direct_and_transitive_references() {
        let dir = make_tiny_sidecar();
        let ours = empty_vanilla_data();
        let tags = load(dir.path(), &ours).unwrap();
        let item_reg = tags
            .registries
            .get(&Identifier::parse("minecraft:item").unwrap())
            .expect("item tags present");
        let snacks = item_reg
            .get(&Identifier::parse("minecraft:food/snacks").unwrap())
            .unwrap();
        assert_eq!(snacks, &[5]);
        let everything = item_reg
            .get(&Identifier::parse("minecraft:everything").unwrap())
            .unwrap();
        assert_eq!(everything, &[5, 7], "sorted, deduped, transitive");
        let hopeful = item_reg
            .get(&Identifier::parse("minecraft:hopeful").unwrap())
            .unwrap();
        assert!(
            hopeful.is_empty(),
            "missing optional entry yields empty tag"
        );
    }

    #[test]
    fn cycles_resolve_to_empty_without_panic() {
        let dir = TempDir::new().unwrap();
        let reports = dir.path().join("reports");
        fs::create_dir_all(&reports).unwrap();
        fs::write(reports.join("registries.json"), "{}").unwrap();
        let tags_item = dir
            .path()
            .join("data")
            .join("minecraft")
            .join("tags")
            .join("item");
        fs::create_dir_all(&tags_item).unwrap();
        fs::write(
            tags_item.join("a.json"),
            r##"{ "values": [ "#minecraft:b" ] }"##,
        )
        .unwrap();
        fs::write(
            tags_item.join("b.json"),
            r##"{ "values": [ "#minecraft:a" ] }"##,
        )
        .unwrap();

        let ours = empty_vanilla_data();
        let tags = load(dir.path(), &ours).unwrap();
        let item_reg = tags
            .registries
            .get(&Identifier::parse("minecraft:item").unwrap())
            .unwrap();
        assert!(
            item_reg
                .get(&Identifier::parse("minecraft:a").unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn missing_registries_json_is_reported_clearly() {
        let dir = TempDir::new().unwrap();
        let ours = empty_vanilla_data();
        let err = load(dir.path(), &ours).unwrap_err();
        assert!(matches!(err, TagError::RegistriesMissing(_)));
    }
}
