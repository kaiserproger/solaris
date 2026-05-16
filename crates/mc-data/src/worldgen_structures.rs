//! Narrow worldgen structure fact reader.
//!
//! This loader extracts structure-set references and spacing scalars that
//! Solaris can feed into its own placement policy. It does not execute vanilla
//! structure placement, jigsaw, or processor logic.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

#[derive(Debug, Error)]
pub enum StructureDataError {
    #[error("worldgen structure_set directory not found at {0}")]
    MissingStructureSetDir(PathBuf),
    #[error("structure data io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("structure data parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureSetFacts {
    pub id: Identifier,
    pub structures: Vec<Identifier>,
    pub placement_type: Option<Identifier>,
    pub spacing: Option<i32>,
    pub separation: Option<i32>,
    pub salt: Option<u64>,
}

pub fn load_structure_set_facts(
    worldgen_dir: impl AsRef<Path>,
) -> Result<Vec<StructureSetFacts>, StructureDataError> {
    let set_dir = worldgen_dir.as_ref().join("structure_set");
    if !set_dir.is_dir() {
        return Err(StructureDataError::MissingStructureSetDir(set_dir));
    }

    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&set_dir).map_err(|source| StructureDataError::Io {
        path: set_dir.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| StructureDataError::Io {
            path: set_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();

    paths.into_iter().map(load_one_structure_set_fact).collect()
}

fn load_one_structure_set_fact(path: PathBuf) -> Result<StructureSetFacts, StructureDataError> {
    let id = id_from_file(&path)?;
    let raw: RawStructureSet = read_json(&path)?;
    let structures = raw
        .structures
        .into_iter()
        .map(|entry| parse_id(&path, entry.structure()))
        .collect::<Result<Vec<_>, _>>()?;
    let placement_type = raw
        .placement
        .type_id
        .map(|value| parse_id(&path, value))
        .transpose()?;

    Ok(StructureSetFacts {
        id,
        structures,
        placement_type,
        spacing: raw.placement.spacing,
        separation: raw.placement.separation,
        salt: raw.placement.salt,
    })
}

#[derive(Deserialize)]
struct RawStructureSet {
    #[serde(default)]
    structures: Vec<RawStructureEntry>,
    #[serde(default)]
    placement: RawStructurePlacement,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawStructureEntry {
    Id(String),
    Weighted { structure: String },
}

impl RawStructureEntry {
    fn structure(self) -> String {
        match self {
            Self::Id(structure) | Self::Weighted { structure } => structure,
        }
    }
}

#[derive(Default, Deserialize)]
struct RawStructurePlacement {
    #[serde(rename = "type")]
    type_id: Option<String>,
    spacing: Option<i32>,
    separation: Option<i32>,
    salt: Option<u64>,
}

fn id_from_file(path: &Path) -> Result<Identifier, StructureDataError> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| StructureDataError::InvalidIdentifier {
            path: path.to_path_buf(),
            value: path.display().to_string(),
        })?;
    parse_id(path, format!("minecraft:{stem}"))
}

fn parse_id(path: &Path, value: String) -> Result<Identifier, StructureDataError> {
    Identifier::parse(&value).map_err(|_| StructureDataError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StructureDataError> {
    let bytes = std::fs::read(path).map_err(|source| StructureDataError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| StructureDataError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn loads_synthetic_structure_set_facts() {
        let root = std::env::temp_dir().join(format!(
            "solaris-structure-facts-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        write(
            &root.join("structure_set/villages.json"),
            r#"{
              "structures": [{ "structure": "minecraft:village_plains", "weight": 1 }],
              "placement": {
                "type": "minecraft:random_spread",
                "spacing": 34,
                "separation": 8,
                "salt": 10387312
              }
            }"#,
        );

        let facts = load_structure_set_facts(&root).unwrap();

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].id.as_str(), "minecraft:villages");
        assert_eq!(facts[0].structures[0].as_str(), "minecraft:village_plains");
        assert_eq!(
            facts[0].placement_type.as_ref().map(Identifier::as_str),
            Some("minecraft:random_spread")
        );
        assert_eq!(facts[0].spacing, Some(34));
        assert_eq!(facts[0].separation, Some(8));
        assert_eq!(facts[0].salt, Some(10_387_312));

        let _ = std::fs::remove_dir_all(root);
    }
}
