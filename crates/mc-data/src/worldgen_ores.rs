//! Vanilla worldgen ore feature reader.
//!
//! This parses the data-pack JSON shape into plain Solaris data. It does
//! not imply vanilla terrain parity; `mc-worldgen` still decides how to
//! translate these specs into its own generator rules.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

#[derive(Debug, Error)]
pub enum OreDataError {
    #[error("worldgen placed_feature directory not found at {0}")]
    MissingPlacedFeatureDir(PathBuf),
    #[error("worldgen configured_feature directory not found at {0}")]
    MissingConfiguredFeatureDir(PathBuf),
    #[error("ore data io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ore data parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("configured ore feature {feature} referenced by {placed} is missing at {path}")]
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
pub struct OreFeature {
    pub placed_feature: Identifier,
    pub configured_feature: Identifier,
    pub placement: OrePlacement,
    pub size: u32,
    pub discard_chance_on_air_exposure: f64,
    pub targets: Vec<OreTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrePlacement {
    pub count: Option<OrePlacementCount>,
    pub rarity_chance: Option<u32>,
    pub height: Option<HeightRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrePlacementCount {
    Constant(u32),
    Uniform { min: u32, max: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OreTarget {
    pub state: Identifier,
    pub replaceable_tag: Option<Identifier>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightRange {
    pub kind: Identifier,
    pub min: HeightAnchor,
    pub max: HeightAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

/// Load every `ore_*.json` placed feature under `<worldgen>/placed_feature`
/// and resolve its paired configured feature under `<worldgen>/configured_feature`.
pub fn load_ore_features(worldgen_dir: impl AsRef<Path>) -> Result<Vec<OreFeature>, OreDataError> {
    let worldgen_dir = worldgen_dir.as_ref();
    let placed_dir = worldgen_dir.join("placed_feature");
    let configured_dir = worldgen_dir.join("configured_feature");
    if !placed_dir.is_dir() {
        return Err(OreDataError::MissingPlacedFeatureDir(placed_dir));
    }
    if !configured_dir.is_dir() {
        return Err(OreDataError::MissingConfiguredFeatureDir(configured_dir));
    }

    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&placed_dir).map_err(|source| OreDataError::Io {
        path: placed_dir.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| OreDataError::Io {
            path: placed_dir.clone(),
            source,
        })?;
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") && stem.starts_with("ore_")
        {
            paths.push(path);
        }
    }
    paths.sort();

    paths
        .into_iter()
        .map(|path| load_one_ore_feature(&path, &configured_dir))
        .collect()
}

fn load_one_ore_feature(
    placed_path: &Path,
    configured_dir: &Path,
) -> Result<OreFeature, OreDataError> {
    let placed_feature = id_from_file(placed_path)?;
    let placed: RawPlacedFeature = read_json(placed_path)?;
    let configured_feature = parse_id(placed_path, placed.feature)?;
    if configured_feature.namespace() != "minecraft" {
        return Err(OreDataError::UnsupportedNamespace {
            placed: placed_feature,
            feature: configured_feature,
        });
    }
    let configured_path = configured_dir.join(format!("{}.json", configured_feature.path()));
    if !configured_path.is_file() {
        return Err(OreDataError::MissingConfiguredFeature {
            placed: placed_feature,
            feature: configured_feature,
            path: configured_path,
        });
    }
    let configured: RawConfiguredFeature = read_json(&configured_path)?;

    let placement = parse_placement(placed_path, placed.placement)?;
    let targets = configured
        .config
        .targets
        .into_iter()
        .map(|target| {
            let state = parse_id(&configured_path, target.state.name)?;
            let replaceable_tag = target
                .target
                .tag
                .map(|tag| parse_id(&configured_path, tag))
                .transpose()?;
            Ok(OreTarget {
                state,
                replaceable_tag,
            })
        })
        .collect::<Result<Vec<_>, OreDataError>>()?;

    Ok(OreFeature {
        placed_feature,
        configured_feature,
        placement,
        size: configured.config.size,
        discard_chance_on_air_exposure: configured.config.discard_chance_on_air_exposure,
        targets,
    })
}

fn parse_placement(
    path: &Path,
    placement: Vec<RawPlacement>,
) -> Result<OrePlacement, OreDataError> {
    let mut count = None;
    let mut rarity_chance = None;
    let mut height = None;
    for entry in placement {
        match entry.kind.as_str() {
            "minecraft:count" => count = entry.count,
            "minecraft:rarity_filter" => rarity_chance = entry.chance,
            "minecraft:height_range" => {
                height = entry
                    .height
                    .map(|height| parse_height_range(path, height))
                    .transpose()?;
            }
            _ => {}
        }
    }
    Ok(OrePlacement {
        count: count.map(Into::into),
        rarity_chance,
        height,
    })
}

fn parse_height_range(path: &Path, raw: RawHeightRange) -> Result<HeightRange, OreDataError> {
    Ok(HeightRange {
        kind: parse_id(path, raw.kind)?,
        min: parse_height_anchor(path, raw.min_inclusive)?,
        max: parse_height_anchor(path, raw.max_inclusive)?,
    })
}

fn parse_height_anchor(path: &Path, raw: RawHeightAnchor) -> Result<HeightAnchor, OreDataError> {
    let mut anchor = None;
    for candidate in [
        raw.absolute.map(HeightAnchor::Absolute),
        raw.above_bottom.map(HeightAnchor::AboveBottom),
        raw.below_top.map(HeightAnchor::BelowTop),
    ]
    .into_iter()
    .flatten()
    {
        if anchor.replace(candidate).is_some() {
            return Err(OreDataError::InvalidHeightAnchor {
                path: path.to_path_buf(),
            });
        }
    }
    anchor.ok_or_else(|| OreDataError::InvalidHeightAnchor {
        path: path.to_path_buf(),
    })
}

fn id_from_file(path: &Path) -> Result<Identifier, OreDataError> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| OreDataError::InvalidIdentifier {
            path: path.to_path_buf(),
            value: path.display().to_string(),
        })?;
    parse_id(path, format!("minecraft:{stem}"))
}

fn parse_id(path: &Path, value: String) -> Result<Identifier, OreDataError> {
    Identifier::parse(value.clone()).map_err(|_| OreDataError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, OreDataError> {
    let bytes = std::fs::read(path).map_err(|source| OreDataError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| OreDataError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Deserialize)]
struct RawPlacedFeature {
    feature: String,
    placement: Vec<RawPlacement>,
}

#[derive(Deserialize)]
struct RawPlacement {
    #[serde(rename = "type")]
    kind: String,
    count: Option<RawCount>,
    chance: Option<u32>,
    height: Option<RawHeightRange>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawCount {
    Constant(u32),
    Uniform {
        #[serde(rename = "type")]
        _kind: String,
        min_inclusive: u32,
        max_inclusive: u32,
    },
}

impl From<RawCount> for OrePlacementCount {
    fn from(value: RawCount) -> Self {
        match value {
            RawCount::Constant(count) => Self::Constant(count),
            RawCount::Uniform {
                min_inclusive,
                max_inclusive,
                ..
            } => Self::Uniform {
                min: min_inclusive,
                max: max_inclusive,
            },
        }
    }
}

#[derive(Deserialize)]
struct RawHeightRange {
    #[serde(rename = "type")]
    kind: String,
    min_inclusive: RawHeightAnchor,
    max_inclusive: RawHeightAnchor,
}

#[derive(Deserialize)]
struct RawHeightAnchor {
    absolute: Option<i32>,
    above_bottom: Option<i32>,
    below_top: Option<i32>,
}

#[derive(Deserialize)]
struct RawConfiguredFeature {
    #[serde(rename = "type")]
    _kind: String,
    config: RawOreConfig,
}

#[derive(Deserialize)]
struct RawOreConfig {
    discard_chance_on_air_exposure: f64,
    size: u32,
    targets: Vec<RawOreTarget>,
}

#[derive(Deserialize)]
struct RawOreTarget {
    state: RawBlockState,
    target: RawTargetPredicate,
}

#[derive(Deserialize)]
struct RawBlockState {
    #[serde(rename = "Name")]
    name: String,
}

#[derive(Deserialize)]
struct RawTargetPredicate {
    tag: Option<String>,
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
    fn loads_synthetic_ore_feature_pair() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("placed_feature/ore_test.json"),
            r#"{
              "feature": "minecraft:ore_test",
              "placement": [
                { "type": "minecraft:count", "count": 7 },
                { "type": "minecraft:height_range", "height": {
                  "type": "minecraft:trapezoid",
                  "min_inclusive": { "absolute": -64 },
                  "max_inclusive": { "above_bottom": 80 }
                }}
              ]
            }"#,
        );
        write(
            &root.join("configured_feature/ore_test.json"),
            r#"{
              "type": "minecraft:ore",
              "config": {
                "discard_chance_on_air_exposure": 0.5,
                "size": 4,
                "targets": [
                  { "state": { "Name": "minecraft:test_ore" }, "target": { "tag": "minecraft:stone_ore_replaceables" } },
                  { "state": { "Name": "minecraft:deepslate_test_ore" }, "target": { "tag": "minecraft:deepslate_ore_replaceables" } }
                ]
              }
            }"#,
        );

        let features = load_ore_features(root).unwrap();
        assert_eq!(features.len(), 1);
        let feature = &features[0];
        assert_eq!(feature.placed_feature.as_str(), "minecraft:ore_test");
        assert_eq!(feature.configured_feature.as_str(), "minecraft:ore_test");
        assert_eq!(
            feature.placement.count,
            Some(OrePlacementCount::Constant(7))
        );
        assert_eq!(feature.size, 4);
        assert_eq!(feature.targets.len(), 2);
        assert_eq!(feature.targets[0].state.as_str(), "minecraft:test_ore");
        assert_eq!(
            feature.placement.height.as_ref().unwrap().min,
            HeightAnchor::Absolute(-64)
        );
        assert_eq!(
            feature.placement.height.as_ref().unwrap().max,
            HeightAnchor::AboveBottom(80)
        );
    }

    #[test]
    fn loads_real_diamond_ore_when_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("data/vanilla/data/minecraft/worldgen");
        if !root.is_dir() {
            eprintln!("skipping: {} missing", root.display());
            return;
        }

        let features = load_ore_features(root).unwrap();
        let diamond = features
            .iter()
            .find(|feature| feature.placed_feature.as_str() == "minecraft:ore_diamond")
            .expect("ore_diamond placed feature");

        assert_eq!(
            diamond.configured_feature.as_str(),
            "minecraft:ore_diamond_small"
        );
        assert_eq!(
            diamond.placement.count,
            Some(OrePlacementCount::Constant(7))
        );
        assert_eq!(diamond.size, 4);
        assert!(
            diamond
                .targets
                .iter()
                .any(|target| target.state.as_str() == "minecraft:diamond_ore")
        );
        assert!(
            diamond
                .targets
                .iter()
                .any(|target| target.state.as_str() == "minecraft:deepslate_diamond_ore")
        );
    }
}
