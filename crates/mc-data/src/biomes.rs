//! Minimal biome JSON reader for spawn rules used by Solaris worldgen.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

#[derive(Debug, Error)]
pub enum BiomeDataError {
    #[error("biome directory not found at {0}")]
    Missing(PathBuf),
    #[error("biome file io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("biome file parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("biome tag parse error at {path}: {source}")]
    TagParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid biome or entity identifier {0:?}")]
    InvalidIdentifier(String),
}

#[derive(Debug, Clone, Default)]
pub struct BiomeWorldgenData {
    features_by_biome: BTreeMap<Identifier, Vec<Identifier>>,
    tags: BTreeMap<Identifier, Vec<Identifier>>,
}

impl BiomeWorldgenData {
    #[must_use]
    pub fn from_parts(
        features_by_biome: BTreeMap<Identifier, Vec<Identifier>>,
        tags: BTreeMap<Identifier, Vec<Identifier>>,
    ) -> Self {
        Self {
            features_by_biome,
            tags,
        }
    }

    #[must_use]
    pub fn biomes(&self) -> impl Iterator<Item = &Identifier> {
        self.features_by_biome.keys()
    }

    #[must_use]
    pub fn tag(&self, tag: &Identifier) -> &[Identifier] {
        self.tags.get(tag).map(Vec::as_slice).unwrap_or(&[])
    }

    #[must_use]
    pub fn tags_len(&self) -> usize {
        self.tags.len()
    }

    #[must_use]
    pub fn biomes_for_feature(&self, feature: &Identifier) -> Vec<Identifier> {
        self.features_by_biome
            .iter()
            .filter(|(_, features)| features.contains(feature))
            .map(|(biome, _)| biome.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiomeSpawnEntry {
    pub entity_type: Identifier,
    pub min_count: u32,
    pub max_count: u32,
    pub weight: u32,
}

#[derive(Debug, Clone, Default)]
pub struct BiomeSpawnRules {
    by_biome: BTreeMap<Identifier, BTreeMap<String, Vec<BiomeSpawnEntry>>>,
}

impl BiomeSpawnRules {
    #[must_use]
    pub fn from_entries(
        entries: BTreeMap<Identifier, BTreeMap<String, Vec<BiomeSpawnEntry>>>,
    ) -> Self {
        Self { by_biome: entries }
    }

    #[must_use]
    pub fn entries(&self, biome: &Identifier, group: &str) -> &[BiomeSpawnEntry] {
        self.by_biome
            .get(biome)
            .and_then(|groups| groups.get(group))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_biome.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_biome.is_empty()
    }
}

pub fn load_biome_spawn_rules(path: impl AsRef<Path>) -> Result<BiomeSpawnRules, BiomeDataError> {
    let path = path.as_ref();
    if !path.is_dir() {
        return Err(BiomeDataError::Missing(path.to_path_buf()));
    }
    let mut entries = BTreeMap::new();
    for dir_entry in std::fs::read_dir(path).map_err(|source| BiomeDataError::Io {
        path: path.to_path_buf(),
        source,
    })? {
        let dir_entry = dir_entry.map_err(|source| BiomeDataError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let file_path = dir_entry.path();
        if file_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let stem = file_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| BiomeDataError::InvalidIdentifier(file_path.display().to_string()))?;
        let biome = Identifier::parse(format!("minecraft:{stem}"))
            .map_err(|_| BiomeDataError::InvalidIdentifier(stem.to_string()))?;
        let bytes = std::fs::read(&file_path).map_err(|source| BiomeDataError::Io {
            path: file_path.clone(),
            source,
        })?;
        let raw: RawBiome =
            serde_json::from_slice(&bytes).map_err(|source| BiomeDataError::Parse {
                path: file_path.clone(),
                source,
            })?;
        let groups = raw
            .spawners
            .into_iter()
            .map(|(group, spawns)| {
                let spawns = spawns
                    .into_iter()
                    .map(|spawn| {
                        let entity_type =
                            Identifier::parse(spawn.entity_type.clone()).map_err(|_| {
                                BiomeDataError::InvalidIdentifier(spawn.entity_type.clone())
                            })?;
                        Ok(BiomeSpawnEntry {
                            entity_type,
                            min_count: spawn.min_count,
                            max_count: spawn.max_count,
                            weight: spawn.weight,
                        })
                    })
                    .collect::<Result<Vec<_>, BiomeDataError>>()?;
                Ok((group, spawns))
            })
            .collect::<Result<BTreeMap<_, _>, BiomeDataError>>()?;
        entries.insert(biome, groups);
    }
    Ok(BiomeSpawnRules::from_entries(entries))
}

pub fn load_biome_worldgen_data(
    biome_dir: impl AsRef<Path>,
    biome_tags_dir: impl AsRef<Path>,
) -> Result<BiomeWorldgenData, BiomeDataError> {
    let biome_dir = biome_dir.as_ref();
    if !biome_dir.is_dir() {
        return Err(BiomeDataError::Missing(biome_dir.to_path_buf()));
    }

    let mut features_by_biome = BTreeMap::new();
    for dir_entry in std::fs::read_dir(biome_dir).map_err(|source| BiomeDataError::Io {
        path: biome_dir.to_path_buf(),
        source,
    })? {
        let dir_entry = dir_entry.map_err(|source| BiomeDataError::Io {
            path: biome_dir.to_path_buf(),
            source,
        })?;
        let file_path = dir_entry.path();
        if file_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let biome = id_from_file(&file_path)?;
        let bytes = std::fs::read(&file_path).map_err(|source| BiomeDataError::Io {
            path: file_path.clone(),
            source,
        })?;
        let raw: RawBiome =
            serde_json::from_slice(&bytes).map_err(|source| BiomeDataError::Parse {
                path: file_path.clone(),
                source,
            })?;
        let features = raw
            .features
            .into_iter()
            .flatten()
            .map(|feature| {
                Identifier::parse(feature.clone())
                    .map_err(|_| BiomeDataError::InvalidIdentifier(feature))
            })
            .collect::<Result<Vec<_>, _>>()?;
        features_by_biome.insert(biome, features);
    }

    let tags = load_biome_tags(biome_tags_dir.as_ref())?;
    Ok(BiomeWorldgenData {
        features_by_biome,
        tags,
    })
}

#[derive(Deserialize)]
struct RawBiome {
    #[serde(default)]
    spawners: BTreeMap<String, Vec<RawSpawnEntry>>,
    #[serde(default)]
    features: Vec<Vec<String>>,
}

#[derive(Deserialize)]
struct RawBiomeTag {
    #[serde(default)]
    values: Vec<RawBiomeTagValue>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawBiomeTagValue {
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

fn id_from_file(path: &Path) -> Result<Identifier, BiomeDataError> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| BiomeDataError::InvalidIdentifier(path.display().to_string()))?;
    Identifier::parse(format!("minecraft:{stem}"))
        .map_err(|_| BiomeDataError::InvalidIdentifier(stem.to_string()))
}

fn load_biome_tags(path: &Path) -> Result<BTreeMap<Identifier, Vec<Identifier>>, BiomeDataError> {
    if !path.is_dir() {
        return Ok(BTreeMap::new());
    }
    let mut raw = BTreeMap::new();
    collect_tag_files(path, path, &mut raw)?;

    let mut resolved = BTreeMap::new();
    for tag_path in raw.keys() {
        let mut visiting = BTreeSet::new();
        let mut values = BTreeSet::new();
        resolve_tag(tag_path, &raw, &mut visiting, &mut values);
        let tag = Identifier::parse(format!("minecraft:{tag_path}"))
            .map_err(|_| BiomeDataError::InvalidIdentifier(tag_path.clone()))?;
        resolved.insert(tag, values.into_iter().collect());
    }
    Ok(resolved)
}

fn collect_tag_files(
    root: &Path,
    dir: &Path,
    raw: &mut BTreeMap<String, (PathBuf, RawBiomeTag)>,
) -> Result<(), BiomeDataError> {
    for entry in std::fs::read_dir(dir).map_err(|source| BiomeDataError::Io {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| BiomeDataError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let ty = entry.file_type().map_err(|source| BiomeDataError::Io {
            path: path.clone(),
            source,
        })?;
        if ty.is_dir() {
            collect_tag_files(root, &path, raw)?;
        } else if ty.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            let rel = path
                .strip_prefix(root)
                .expect("walk yields paths under root")
                .with_extension("");
            let joined = rel
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let body = std::fs::read_to_string(&path).map_err(|source| BiomeDataError::Io {
                path: path.clone(),
                source,
            })?;
            let parsed: RawBiomeTag =
                serde_json::from_str(&body).map_err(|source| BiomeDataError::TagParse {
                    path: path.clone(),
                    source,
                })?;
            raw.insert(joined, (path, parsed));
        }
    }
    Ok(())
}

fn resolve_tag(
    tag_path: &str,
    raw: &BTreeMap<String, (PathBuf, RawBiomeTag)>,
    visiting: &mut BTreeSet<String>,
    values: &mut BTreeSet<Identifier>,
) {
    if !visiting.insert(tag_path.to_string()) {
        return;
    }
    let Some((_, tag)) = raw.get(tag_path) else {
        visiting.remove(tag_path);
        return;
    };
    for value in &tag.values {
        let (id, required) = match value {
            RawBiomeTagValue::Plain(id) => (id.as_str(), true),
            RawBiomeTagValue::Object { id, required } => (id.as_str(), *required),
        };
        if let Some(inner) = id.strip_prefix('#') {
            let inner = inner
                .strip_prefix("minecraft:")
                .unwrap_or_else(|| inner.split_once(':').map_or(inner, |(_, path)| path));
            resolve_tag(inner, raw, visiting, values);
        } else if let Ok(id) = Identifier::parse(id.to_string()) {
            values.insert(id);
        } else if required {
            values.clear();
        }
    }
    visiting.remove(tag_path);
}

#[derive(Deserialize)]
struct RawSpawnEntry {
    #[serde(rename = "type")]
    entity_type: String,
    #[serde(rename = "minCount")]
    min_count: u32,
    #[serde(rename = "maxCount")]
    max_count: u32,
    weight: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_real_plains_spawns_when_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("data/vanilla/data/minecraft/worldgen/biome");
        if !root.is_dir() {
            eprintln!("skipping: {} missing", root.display());
            return;
        }

        let rules = load_biome_spawn_rules(root).unwrap();
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let creature = rules.entries(&plains, "creature");

        assert!(
            creature
                .iter()
                .any(|entry| entry.entity_type.as_str() == "minecraft:pig")
        );
        assert!(
            creature
                .iter()
                .any(|entry| entry.entity_type.as_str() == "minecraft:chicken")
        );
        assert!(
            creature
                .iter()
                .any(|entry| entry.entity_type.as_str() == "minecraft:cow")
        );
    }

    #[test]
    fn loads_biome_features_and_identifier_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let biome_dir = tmp.path().join("worldgen/biome");
        let tags_dir = tmp.path().join("tags/worldgen/biome");
        std::fs::create_dir_all(&biome_dir).unwrap();
        std::fs::create_dir_all(&tags_dir).unwrap();
        std::fs::write(
            biome_dir.join("plains.json"),
            r#"{
              "features": [
                ["minecraft:ore_diamond"],
                ["minecraft:patch_grass", "minecraft:flower_plain"]
              ],
              "spawners": {}
            }"#,
        )
        .unwrap();
        std::fs::write(
            biome_dir.join("forest.json"),
            r#"{ "features": [["minecraft:ore_diamond"]], "spawners": {} }"#,
        )
        .unwrap();
        std::fs::write(
            tags_dir.join("is_overworld.json"),
            r#"{ "values": ["minecraft:plains", "minecraft:forest"] }"#,
        )
        .unwrap();
        std::fs::write(
            tags_dir.join("is_forest.json"),
            r##"{ "values": ["#minecraft:forest_like"] }"##,
        )
        .unwrap();
        std::fs::write(
            tags_dir.join("forest_like.json"),
            r#"{ "values": ["minecraft:forest"] }"#,
        )
        .unwrap();

        let data = load_biome_worldgen_data(&biome_dir, &tags_dir).unwrap();
        let diamond = Identifier::parse("minecraft:ore_diamond").unwrap();
        let biomes = data.biomes_for_feature(&diamond);

        assert_eq!(biomes.len(), 2);
        assert!(biomes.contains(&Identifier::parse("minecraft:plains").unwrap()));
        assert_eq!(
            data.tag(&Identifier::parse("minecraft:is_forest").unwrap()),
            &[Identifier::parse("minecraft:forest").unwrap()]
        );
    }

    #[test]
    fn loads_real_overworld_biome_tags_when_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("data/vanilla/data/minecraft");
        let biome_dir = root.join("worldgen/biome");
        let tags_dir = root.join("tags/worldgen/biome");
        if !biome_dir.is_dir() || !tags_dir.is_dir() {
            eprintln!("skipping: biome sidecar missing under {}", root.display());
            return;
        }

        let data = load_biome_worldgen_data(&biome_dir, &tags_dir).unwrap();
        let overworld = data.tag(&Identifier::parse("minecraft:is_overworld").unwrap());

        assert!(overworld.contains(&Identifier::parse("minecraft:plains").unwrap()));
        assert!(overworld.contains(&Identifier::parse("minecraft:deep_dark").unwrap()));
        assert!(overworld.contains(&Identifier::parse("minecraft:cherry_grove").unwrap()));
        assert!(overworld.len() >= 50);
    }
}
