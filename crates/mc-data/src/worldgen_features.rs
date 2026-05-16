//! Narrow worldgen feature fact reader.
//!
//! This parses placed/configured feature JSON into scalar facts Solaris can feed
//! into independent generation policy. It does not execute vanilla feature or
//! placement algorithms.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use crate::Identifier;

#[derive(Debug, Error)]
pub enum FeatureDataError {
    #[error("worldgen placed_feature directory not found at {0}")]
    MissingPlacedFeatureDir(PathBuf),
    #[error("worldgen configured_feature directory not found at {0}")]
    MissingConfiguredFeatureDir(PathBuf),
    #[error("feature data io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("feature data parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("configured feature {feature} referenced by {placed} is missing at {path}")]
    MissingConfiguredFeature {
        placed: Identifier,
        feature: Identifier,
        path: PathBuf,
    },
    #[error("unsupported non-minecraft configured feature {feature} referenced by {placed}")]
    UnsupportedNamespace {
        placed: Identifier,
        feature: Identifier,
    },
    #[error("height anchor in {path} must contain exactly one anchor kind")]
    InvalidHeightAnchor { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorldgenFeatureFacts {
    pub placed_feature: Identifier,
    pub configured_feature: Identifier,
    pub configured_type: Identifier,
    pub placement: FeaturePlacementFacts,
    pub block_states: Vec<Identifier>,
    pub tags: Vec<Identifier>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeaturePlacementFacts {
    pub modifiers: Vec<Identifier>,
    pub count: Option<FeatureCount>,
    pub rarity_chance: Option<u32>,
    pub height: Option<FeatureHeightRange>,
    pub heightmap: Option<String>,
    pub has_biome_filter: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureCount {
    Constant(u32),
    Uniform { min: u32, max: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureHeightRange {
    pub kind: Identifier,
    pub min: FeatureHeightAnchor,
    pub max: FeatureHeightAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureHeightAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

pub fn load_feature_facts(
    worldgen_dir: impl AsRef<Path>,
) -> Result<Vec<WorldgenFeatureFacts>, FeatureDataError> {
    let worldgen_dir = worldgen_dir.as_ref();
    let placed_dir = worldgen_dir.join("placed_feature");
    let configured_dir = worldgen_dir.join("configured_feature");
    if !placed_dir.is_dir() {
        return Err(FeatureDataError::MissingPlacedFeatureDir(placed_dir));
    }
    if !configured_dir.is_dir() {
        return Err(FeatureDataError::MissingConfiguredFeatureDir(
            configured_dir,
        ));
    }

    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&placed_dir).map_err(|source| FeatureDataError::Io {
        path: placed_dir.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| FeatureDataError::Io {
            path: placed_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();

    paths
        .into_iter()
        .map(|path| load_one_feature_fact(&path, &configured_dir))
        .collect()
}

fn load_one_feature_fact(
    placed_path: &Path,
    configured_dir: &Path,
) -> Result<WorldgenFeatureFacts, FeatureDataError> {
    let placed_feature = id_from_file(placed_path)?;
    let placed: RawPlacedFeature = read_json(placed_path)?;
    let configured_feature = parse_id(placed_path, placed.feature)?;
    if configured_feature.namespace() != "minecraft" {
        return Err(FeatureDataError::UnsupportedNamespace {
            placed: placed_feature,
            feature: configured_feature,
        });
    }

    let configured_path = configured_dir.join(format!("{}.json", configured_feature.path()));
    if !configured_path.is_file() {
        return Err(FeatureDataError::MissingConfiguredFeature {
            placed: placed_feature,
            feature: configured_feature,
            path: configured_path,
        });
    }
    let configured_value: Value = read_json(&configured_path)?;
    let configured_type = configured_value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| FeatureDataError::InvalidIdentifier {
            path: configured_path.clone(),
            value: String::from("<missing configured feature type>"),
        })?;
    let configured_type = parse_id(&configured_path, configured_type.to_string())?;

    let placement = parse_placement(placed_path, &placed.placement)?;
    let mut block_states = BTreeSet::new();
    collect_block_states(&configured_value, &mut block_states);
    let mut tags = BTreeSet::new();
    for entry in &placed.placement {
        collect_tags(entry, &mut tags);
    }
    collect_tags(&configured_value, &mut tags);

    Ok(WorldgenFeatureFacts {
        placed_feature,
        configured_feature,
        configured_type,
        placement,
        block_states: block_states.into_iter().collect(),
        tags: tags.into_iter().collect(),
    })
}

fn parse_placement(
    path: &Path,
    placement: &[Value],
) -> Result<FeaturePlacementFacts, FeatureDataError> {
    let mut facts = FeaturePlacementFacts::default();
    for entry in placement {
        let Some(kind) = entry.get("type").and_then(Value::as_str) else {
            continue;
        };
        let kind = parse_id(path, kind.to_string())?;
        if kind.as_str() == "minecraft:count" {
            facts.count = entry.get("count").and_then(parse_count);
        } else if kind.as_str() == "minecraft:rarity_filter" {
            facts.rarity_chance = entry
                .get("chance")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok());
        } else if kind.as_str() == "minecraft:height_range" {
            facts.height = entry
                .get("height")
                .map(|height| parse_height_range(path, height))
                .transpose()?;
        } else if kind.as_str() == "minecraft:heightmap" {
            facts.heightmap = entry
                .get("heightmap")
                .and_then(Value::as_str)
                .map(str::to_string);
        } else if kind.as_str() == "minecraft:biome" {
            facts.has_biome_filter = true;
        }
        facts.modifiers.push(kind);
    }
    Ok(facts)
}

fn parse_count(value: &Value) -> Option<FeatureCount> {
    if let Some(count) = value.as_u64().and_then(|value| u32::try_from(value).ok()) {
        return Some(FeatureCount::Constant(count));
    }
    let object = value.as_object()?;
    let min = object
        .get("min_inclusive")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())?;
    let max = object
        .get("max_inclusive")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())?;
    Some(FeatureCount::Uniform { min, max })
}

fn parse_height_range(path: &Path, value: &Value) -> Result<FeatureHeightRange, FeatureDataError> {
    let kind = value.get("type").and_then(Value::as_str).ok_or_else(|| {
        FeatureDataError::InvalidIdentifier {
            path: path.to_path_buf(),
            value: String::from("<missing height range type>"),
        }
    })?;
    let min = value
        .get("min_inclusive")
        .ok_or_else(|| FeatureDataError::InvalidHeightAnchor {
            path: path.to_path_buf(),
        })?;
    let max = value
        .get("max_inclusive")
        .ok_or_else(|| FeatureDataError::InvalidHeightAnchor {
            path: path.to_path_buf(),
        })?;
    Ok(FeatureHeightRange {
        kind: parse_id(path, kind.to_string())?,
        min: parse_height_anchor(path, min)?,
        max: parse_height_anchor(path, max)?,
    })
}

fn parse_height_anchor(
    path: &Path,
    value: &Value,
) -> Result<FeatureHeightAnchor, FeatureDataError> {
    let object = value
        .as_object()
        .ok_or_else(|| FeatureDataError::InvalidHeightAnchor {
            path: path.to_path_buf(),
        })?;
    let mut anchor = None;
    for (key, build) in [
        (
            "absolute",
            FeatureHeightAnchor::Absolute as fn(i32) -> FeatureHeightAnchor,
        ),
        ("above_bottom", FeatureHeightAnchor::AboveBottom),
        ("below_top", FeatureHeightAnchor::BelowTop),
    ] {
        let Some(value) = object
            .get(key)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
        else {
            continue;
        };
        if anchor.replace(build(value)).is_some() {
            return Err(FeatureDataError::InvalidHeightAnchor {
                path: path.to_path_buf(),
            });
        }
    }
    anchor.ok_or_else(|| FeatureDataError::InvalidHeightAnchor {
        path: path.to_path_buf(),
    })
}

fn collect_block_states(value: &Value, out: &mut BTreeSet<Identifier>) {
    match value {
        Value::Object(object) => {
            if let Some(name) = object.get("Name").and_then(Value::as_str)
                && let Ok(id) = Identifier::parse(name.to_string())
            {
                out.insert(id);
            }
            for child in object.values() {
                collect_block_states(child, out);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_block_states(child, out);
            }
        }
        _ => {}
    }
}

fn collect_tags(value: &Value, out: &mut BTreeSet<Identifier>) {
    match value {
        Value::Object(object) => {
            if let Some(tag) = object.get("tag").and_then(Value::as_str)
                && let Ok(id) = Identifier::parse(tag.to_string())
            {
                out.insert(id);
            }
            for child in object.values() {
                collect_tags(child, out);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_tags(child, out);
            }
        }
        _ => {}
    }
}

fn id_from_file(path: &Path) -> Result<Identifier, FeatureDataError> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| FeatureDataError::InvalidIdentifier {
            path: path.to_path_buf(),
            value: path.display().to_string(),
        })?;
    parse_id(path, format!("minecraft:{stem}"))
}

fn parse_id(path: &Path, value: String) -> Result<Identifier, FeatureDataError> {
    Identifier::parse(value.clone()).map_err(|_| FeatureDataError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, FeatureDataError> {
    let bytes = std::fs::read(path).map_err(|source| FeatureDataError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| FeatureDataError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Deserialize)]
struct RawPlacedFeature {
    feature: String,
    placement: Vec<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn loads_synthetic_worldgen_feature_facts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("placed_feature/patch_test.json"),
            r#"{
              "feature": "minecraft:test_feature",
              "placement": [
                { "type": "minecraft:count", "count": 3 },
                { "type": "minecraft:rarity_filter", "chance": 5 },
                { "type": "minecraft:height_range", "height": {
                  "type": "minecraft:uniform",
                  "min_inclusive": { "absolute": -16 },
                  "max_inclusive": { "below_top": 8 }
                }},
                { "type": "minecraft:heightmap", "heightmap": "WORLD_SURFACE_WG" },
                { "type": "minecraft:biome" },
                { "type": "minecraft:block_predicate_filter", "predicate": { "tag": "minecraft:air" } }
              ]
            }"#,
        );
        write(
            &root.join("configured_feature/test_feature.json"),
            r#"{
              "type": "minecraft:simple_block",
              "config": {
                "to_place": {
                  "type": "minecraft:simple_state_provider",
                  "state": { "Name": "minecraft:test_flower" }
                }
              }
            }"#,
        );

        let features = load_feature_facts(root).unwrap();

        assert_eq!(features.len(), 1);
        let feature = &features[0];
        assert_eq!(feature.placed_feature.as_str(), "minecraft:patch_test");
        assert_eq!(
            feature.configured_feature.as_str(),
            "minecraft:test_feature"
        );
        assert_eq!(feature.configured_type.as_str(), "minecraft:simple_block");
        assert_eq!(feature.placement.count, Some(FeatureCount::Constant(3)));
        assert_eq!(feature.placement.rarity_chance, Some(5));
        assert!(feature.placement.has_biome_filter);
        assert_eq!(
            feature.placement.heightmap.as_deref(),
            Some("WORLD_SURFACE_WG")
        );
        assert_eq!(
            feature.placement.height.as_ref().unwrap().min,
            FeatureHeightAnchor::Absolute(-16)
        );
        assert_eq!(
            feature.placement.height.as_ref().unwrap().max,
            FeatureHeightAnchor::BelowTop(8)
        );
        assert_eq!(
            feature.block_states,
            vec![Identifier::parse("minecraft:test_flower").unwrap()]
        );
        assert_eq!(
            feature.tags,
            vec![Identifier::parse("minecraft:air").unwrap()]
        );
    }

    #[test]
    fn loads_real_grass_feature_facts_when_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("data/vanilla/data/minecraft/worldgen");
        if !root.is_dir() {
            eprintln!("skipping: worldgen sidecar missing at {}", root.display());
            return;
        }

        let features = load_feature_facts(root).unwrap();
        let grass = features
            .iter()
            .find(|feature| feature.placed_feature.as_str() == "minecraft:patch_grass_plain")
            .expect("patch_grass_plain placed feature");

        assert_eq!(grass.configured_feature.as_str(), "minecraft:grass");
        assert_eq!(grass.configured_type.as_str(), "minecraft:simple_block");
        assert_eq!(grass.placement.count, Some(FeatureCount::Constant(32)));
        assert!(grass.placement.has_biome_filter);
        assert_eq!(
            grass.placement.heightmap.as_deref(),
            Some("WORLD_SURFACE_WG")
        );
        assert!(
            grass
                .block_states
                .contains(&Identifier::parse("minecraft:short_grass").unwrap())
        );
        assert!(
            grass
                .tags
                .contains(&Identifier::parse("minecraft:air").unwrap())
        );
    }
}
