use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

const BUILTIN_SURVIVAL_LOOT: &str = include_str!("../data/survival_loot.json");

#[derive(Debug, Error)]
pub enum LootError {
    #[error("loot file {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid loot identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("filesystem error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Default)]
pub struct LootTables {
    entity_drops: BTreeMap<Identifier, Identifier>,
    block_drops: BTreeMap<Identifier, Identifier>,
}

impl LootTables {
    #[must_use]
    pub fn entity_drop(&self, entity: &Identifier) -> Option<&Identifier> {
        self.entity_drops.get(entity)
    }

    #[must_use]
    pub fn block_drop(&self, block: &Identifier) -> Option<&Identifier> {
        self.block_drops.get(block)
    }

    #[must_use]
    pub fn total_drops(&self) -> usize {
        self.entity_drops.len() + self.block_drops.len()
    }
}

#[derive(Deserialize)]
struct RawLootTables {
    #[serde(default)]
    entities: BTreeMap<String, String>,
    #[serde(default)]
    blocks: BTreeMap<String, String>,
}

#[must_use]
pub fn builtin() -> &'static LootTables {
    static BUILTIN: OnceLock<LootTables> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        from_str(
            BUILTIN_SURVIVAL_LOOT,
            Path::new("crates/mc-data/data/survival_loot.json"),
        )
        .expect("built-in Solaris survival loot JSON is valid")
    })
}

pub fn load(path: impl AsRef<Path>) -> Result<LootTables, LootError> {
    let path = path.as_ref();
    let bytes = std::fs::read_to_string(path).map_err(|source| LootError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    from_str(&bytes, path)
}

fn from_str(raw: &str, path: &Path) -> Result<LootTables, LootError> {
    let raw: RawLootTables = serde_json::from_str(raw).map_err(|source| LootError::Malformed {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(LootTables {
        entity_drops: parse_map(path, raw.entities)?,
        block_drops: parse_map(path, raw.blocks)?,
    })
}

fn parse_map(
    path: &Path,
    raw: BTreeMap<String, String>,
) -> Result<BTreeMap<Identifier, Identifier>, LootError> {
    raw.into_iter()
        .map(|(source, drop)| {
            let source_id = parse_id(path, source)?;
            let drop_id = parse_id(path, drop)?;
            Ok((source_id, drop_id))
        })
        .collect()
}

fn parse_id(path: &Path, value: String) -> Result<Identifier, LootError> {
    Identifier::parse(value.clone()).map_err(|_| LootError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn builtin_survival_loot_loads_from_repo_json() {
        let loot = builtin();

        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:cow").unwrap()),
            Some(&Identifier::parse("minecraft:beef").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
    }

    #[test]
    fn loads_custom_repo_owned_loot_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("loot.json");
        fs::write(
            &path,
            r#"{
              "entities": { "minecraft:cow": "minecraft:beef" },
              "blocks": { "minecraft:stone": "minecraft:cobblestone" }
            }"#,
        )
        .unwrap();

        let loot = load(&path).unwrap();

        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:cow").unwrap()),
            Some(&Identifier::parse("minecraft:beef").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
    }
}
