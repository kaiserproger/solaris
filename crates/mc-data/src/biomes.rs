//! Minimal biome JSON reader for spawn rules used by Solaris worldgen.

use std::collections::BTreeMap;
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
    #[error("invalid biome or entity identifier {0:?}")]
    InvalidIdentifier(String),
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

#[derive(Deserialize)]
struct RawBiome {
    #[serde(default)]
    spawners: BTreeMap<String, Vec<RawSpawnEntry>>,
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
}
