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
    pub fn from_maps(
        entity_drops: BTreeMap<Identifier, Identifier>,
        block_drops: BTreeMap<Identifier, Identifier>,
    ) -> Self {
        Self {
            entity_drops,
            block_drops,
        }
    }

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

pub fn load_vanilla_subset(root: impl AsRef<Path>) -> Result<LootTables, LootError> {
    let root = root.as_ref();
    if !root.is_dir() {
        return Ok(LootTables::default());
    }

    let block_drops = load_vanilla_kind(&root.join("blocks"))?;
    let entity_drops = load_vanilla_kind(&root.join("entities"))?;
    Ok(LootTables::from_maps(entity_drops, block_drops))
}

fn load_vanilla_kind(dir: &Path) -> Result<BTreeMap<Identifier, Identifier>, LootError> {
    if !dir.is_dir() {
        return Ok(BTreeMap::new());
    }

    let mut paths = Vec::new();
    collect_json_files(dir, &mut paths)?;
    paths.sort();

    let mut drops = BTreeMap::new();
    for path in paths {
        let source = id_from_file(dir, &path)?;
        let bytes = std::fs::read_to_string(&path).map_err(|source| LootError::Io {
            path: path.clone(),
            source,
        })?;
        let value: serde_json::Value =
            serde_json::from_str(&bytes).map_err(|source| LootError::Malformed {
                path: path.clone(),
                source,
            })?;
        if let Some(drop) = simple_drop_from_table(&path, &value)? {
            drops.insert(source, drop);
        }
    }
    Ok(drops)
}

fn collect_json_files(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), LootError> {
    let entries = std::fs::read_dir(dir).map_err(|source| LootError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| LootError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let ty = entry.file_type().map_err(|source| LootError::Io {
            path: path.clone(),
            source,
        })?;
        if ty.is_dir() {
            collect_json_files(&path, paths)?;
        } else if ty.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            paths.push(path);
        }
    }
    Ok(())
}

fn id_from_file(root: &Path, path: &Path) -> Result<Identifier, LootError> {
    let rel = path
        .strip_prefix(root)
        .expect("walk yields paths under loot root")
        .with_extension("");
    let mut joined = String::from("minecraft:");
    for component in rel.components() {
        if !joined.ends_with(':') {
            joined.push('/');
        }
        joined.push_str(component.as_os_str().to_string_lossy().as_ref());
    }
    parse_id(path, joined)
}

fn simple_drop_from_table(
    path: &Path,
    value: &serde_json::Value,
) -> Result<Option<Identifier>, LootError> {
    let Some(pools) = value.get("pools").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    for pool in pools {
        if has_unsupported_conditions(pool) {
            continue;
        }
        let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for entry in entries {
            if let Some(drop) = simple_drop_from_entry(path, entry)? {
                return Ok(Some(drop));
            }
        }
    }
    Ok(None)
}

fn simple_drop_from_entry(
    path: &Path,
    entry: &serde_json::Value,
) -> Result<Option<Identifier>, LootError> {
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("minecraft:item") => {
            if has_unsupported_conditions(entry) {
                return Ok(None);
            }
            let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
                return Ok(None);
            };
            parse_id(path, name.to_string()).map(Some)
        }
        Some("minecraft:alternatives") => {
            let Some(children) = entry.get("children").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            for child in children {
                if let Some(drop) = simple_drop_from_entry(path, child)? {
                    return Ok(Some(drop));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn has_unsupported_conditions(value: &serde_json::Value) -> bool {
    value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                !matches!(
                    condition
                        .get("condition")
                        .and_then(serde_json::Value::as_str),
                    Some("minecraft:survives_explosion")
                )
            })
        })
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

    #[test]
    fn loads_simple_vanilla_subset_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&blocks).unwrap();
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            blocks.join("stone.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:alternatives",
                  "children": [
                    {
                      "type": "minecraft:item",
                      "conditions": [{ "condition": "minecraft:match_tool" }],
                      "name": "minecraft:stone"
                    },
                    {
                      "type": "minecraft:item",
                      "conditions": [{ "condition": "minecraft:survives_explosion" }],
                      "name": "minecraft:cobblestone"
                    }
                  ]
                }]
              }]
            }"#,
        )
        .unwrap();
        fs::write(
            entities.join("zombie.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{ "function": "minecraft:set_count" }],
                  "name": "minecraft:rotten_flesh"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:zombie").unwrap()),
            Some(&Identifier::parse("minecraft:rotten_flesh").unwrap())
        );
    }

    #[test]
    fn loads_real_vanilla_subset_when_present() {
        let path = workspace_path("data/vanilla/data/minecraft/loot_table");
        if !path.is_dir() {
            eprintln!("skipping: {} not present", path.display());
            return;
        }

        let loot = load_vanilla_subset(path).unwrap();

        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:grass_block").unwrap()),
            Some(&Identifier::parse("minecraft:dirt").unwrap())
        );
        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:zombie").unwrap()),
            Some(&Identifier::parse("minecraft:rotten_flesh").unwrap())
        );
    }

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }
}
