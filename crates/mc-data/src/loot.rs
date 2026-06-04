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
    entity_drops: BTreeMap<Identifier, LootDrop>,
    block_drops: BTreeMap<Identifier, LootDrop>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootDrop {
    pub item: Identifier,
    pub count: u32,
}

impl LootDrop {
    #[must_use]
    pub fn single(item: Identifier) -> Self {
        Self { item, count: 1 }
    }
}

impl LootTables {
    #[must_use]
    pub fn from_maps(
        entity_drops: BTreeMap<Identifier, Identifier>,
        block_drops: BTreeMap<Identifier, Identifier>,
    ) -> Self {
        Self::from_drop_maps(
            entity_drops
                .into_iter()
                .map(|(source, item)| (source, LootDrop::single(item)))
                .collect(),
            block_drops
                .into_iter()
                .map(|(source, item)| (source, LootDrop::single(item)))
                .collect(),
        )
    }

    #[must_use]
    pub fn from_drop_maps(
        entity_drops: BTreeMap<Identifier, LootDrop>,
        block_drops: BTreeMap<Identifier, LootDrop>,
    ) -> Self {
        Self {
            entity_drops,
            block_drops,
        }
    }

    #[must_use]
    pub fn entity_drop(&self, entity: &Identifier) -> Option<&Identifier> {
        self.entity_drop_stack(entity).map(|drop| &drop.item)
    }

    #[must_use]
    pub fn entity_drop_stack(&self, entity: &Identifier) -> Option<&LootDrop> {
        self.entity_drops.get(entity)
    }

    #[must_use]
    pub fn block_drop(&self, block: &Identifier) -> Option<&Identifier> {
        self.block_drop_stack(block).map(|drop| &drop.item)
    }

    #[must_use]
    pub fn block_drop_stack(&self, block: &Identifier) -> Option<&LootDrop> {
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
    Ok(LootTables::from_drop_maps(entity_drops, block_drops))
}

fn load_vanilla_kind(dir: &Path) -> Result<BTreeMap<Identifier, LootDrop>, LootError> {
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
) -> Result<Option<LootDrop>, LootError> {
    let Some(pools) = value.get("pools").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    for pool in pools {
        if has_unsupported_pool_rolls(pool) {
            return Ok(None);
        }
        if has_unsupported_conditions(pool) {
            continue;
        }
        let Some(pool_count) = supported_count_from_functions(pool) else {
            continue;
        };
        let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for entry in entries {
            if let Some(mut drop) = simple_drop_from_entry(path, entry)? {
                if let Some(count) = pool_count {
                    drop.count = count;
                }
                return Ok(Some(drop));
            }
        }
    }
    Ok(None)
}

fn simple_drop_from_entry(
    path: &Path,
    entry: &serde_json::Value,
) -> Result<Option<LootDrop>, LootError> {
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("minecraft:item") => {
            if has_unsupported_conditions(entry) {
                return Ok(None);
            }
            let Some(count) = supported_count_from_functions(entry) else {
                return Ok(None);
            };
            let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
                return Ok(None);
            };
            parse_id(path, name.to_string()).map(|item| {
                Some(LootDrop {
                    item,
                    count: count.unwrap_or(1),
                })
            })
        }
        Some("minecraft:alternatives") => {
            if has_unsupported_conditions(entry) || entry.get("features").is_some() {
                return Ok(None);
            }
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

fn has_unsupported_pool_rolls(pool: &serde_json::Value) -> bool {
    !is_supported_constant_roll(pool.get("rolls"), 1)
        || !is_supported_constant_roll(pool.get("bonus_rolls"), 0)
}

fn is_supported_constant_roll(value: Option<&serde_json::Value>, supported: u32) -> bool {
    let Some(value) = value else {
        return true;
    };
    if let Some(value) = value.as_u64() {
        return value == u64::from(supported);
    }
    value
        .as_f64()
        .is_some_and(|value| value == f64::from(supported))
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

fn supported_count_from_functions(value: &serde_json::Value) -> Option<Option<u32>> {
    let Some(functions) = value.get("functions").and_then(serde_json::Value::as_array) else {
        return Some(None);
    };
    let mut count = None;
    for function in functions {
        let fields = function.as_object()?;
        if fields
            .keys()
            .any(|key| !matches!(key.as_str(), "function" | "count"))
        {
            return None;
        }
        if function.get("function").and_then(serde_json::Value::as_str)
            != Some("minecraft:set_count")
        {
            return None;
        }
        let raw_count = function.get("count")?;
        let parsed = raw_count
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())?;
        if parsed == 0 || parsed > 64 {
            return None;
        }
        count = Some(parsed);
    }
    Some(count)
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
) -> Result<BTreeMap<Identifier, LootDrop>, LootError> {
    raw.into_iter()
        .map(|(source, drop)| {
            let source_id = parse_id(path, source)?;
            let drop_id = parse_id(path, drop)?;
            Ok((source_id, LootDrop::single(drop_id)))
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
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:podzol").unwrap()),
            Some(&Identifier::parse("minecraft:dirt").unwrap())
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
        fs::create_dir_all(entities.join("passive")).unwrap();
        fs::write(
            entities.join("passive").join("cow.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "name": "minecraft:beef"
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
            loot.block_drop_stack(&Identifier::parse("minecraft:stone").unwrap())
                .map(|drop| drop.count),
            Some(1)
        );
        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:passive/cow").unwrap()),
            Some(&Identifier::parse("minecraft:beef").unwrap())
        );
    }

    #[test]
    fn loads_vanilla_subset_set_count_constant() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            entities.join("zombie.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{
                    "function": "minecraft:set_count",
                    "count": 2
                  }],
                  "name": "minecraft:rotten_flesh"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.entity_drop_stack(&Identifier::parse("minecraft:zombie").unwrap()),
            Some(&LootDrop {
                item: Identifier::parse("minecraft:rotten_flesh").unwrap(),
                count: 2,
            })
        );
    }

    #[test]
    fn skips_vanilla_tables_with_unsupported_functions() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            entities.join("zombie.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{ "function": "minecraft:looting_enchant" }],
                  "name": "minecraft:rotten_flesh"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:zombie").unwrap()),
            None
        );
    }

    #[test]
    fn skips_vanilla_tables_with_unsupported_set_count_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            entities.join("zombie.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{
                    "function": "minecraft:set_count",
                    "count": { "min": 0, "max": 2 }
                  }],
                  "name": "minecraft:rotten_flesh"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.entity_drop_stack(&Identifier::parse("minecraft:zombie").unwrap()),
            None
        );
    }

    #[test]
    fn loads_pool_level_set_count_constant() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("oak_leaves.json"),
            r#"{
              "pools": [{
                "functions": [{ "function": "minecraft:set_count", "count": 3 }],
                "entries": [{
                  "type": "minecraft:item",
                  "name": "minecraft:apple"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop_stack(&Identifier::parse("minecraft:oak_leaves").unwrap()),
            Some(&LootDrop {
                item: Identifier::parse("minecraft:apple").unwrap(),
                count: 3,
            })
        );
    }

    #[test]
    fn unsupported_rolls_or_bonus_rolls_fail_closed_for_whole_table() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("bad_rolls.json"),
            r#"{
              "pools": [
                {
                  "rolls": 2,
                  "entries": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
                },
                {
                  "entries": [{ "type": "minecraft:item", "name": "minecraft:cobblestone" }]
                }
              ]
            }"#,
        )
        .unwrap();
        fs::write(
            blocks.join("bad_bonus_rolls.json"),
            r#"{
              "pools": [
                {
                  "bonus_rolls": { "type": "minecraft:uniform", "min": 0, "max": 1 },
                  "entries": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
                },
                {
                  "entries": [{ "type": "minecraft:item", "name": "minecraft:dirt" }]
                }
              ]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:bad_rolls").unwrap()),
            None
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:bad_bonus_rolls").unwrap()),
            None
        );
    }

    #[test]
    fn skips_set_count_functions_with_unsupported_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        for (name, function) in [
            (
                "conditioned",
                r#"{
                  "function": "minecraft:set_count",
                  "conditions": [{ "condition": "minecraft:survives_explosion" }],
                  "count": 2
                }"#,
            ),
            (
                "additive",
                r#"{
                  "function": "minecraft:set_count",
                  "add": true,
                  "count": 2
                }"#,
            ),
            (
                "extra_field",
                r#"{
                  "function": "minecraft:set_count",
                  "count": 2,
                  "quality": 1
                }"#,
            ),
        ] {
            fs::write(
                entities.join(format!("{name}.json")),
                format!(
                    r#"{{
                      "pools": [{{
                        "entries": [{{
                          "type": "minecraft:item",
                          "functions": [{function}],
                          "name": "minecraft:rotten_flesh"
                        }}]
                      }}]
                    }}"#
                ),
            )
            .unwrap();
        }

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        for name in ["conditioned", "additive", "extra_field"] {
            assert_eq!(
                loot.entity_drop_stack(&Identifier::parse(format!("minecraft:{name}")).unwrap()),
                None
            );
        }
    }

    #[test]
    fn skips_alternatives_wrappers_with_unsupported_conditions_or_features() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("conditioned_alternatives.json"),
            r#"{
              "pools": [{
                "entries": [
                  {
                    "type": "minecraft:alternatives",
                    "conditions": [{ "condition": "minecraft:match_tool" }],
                    "children": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
                  },
                  { "type": "minecraft:item", "name": "minecraft:cobblestone" }
                ]
              }]
            }"#,
        )
        .unwrap();
        fs::write(
            blocks.join("featured_alternatives.json"),
            r#"{
              "pools": [{
                "entries": [
                  {
                    "type": "minecraft:alternatives",
                    "features": ["minecraft:update_1_21"],
                    "children": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
                  },
                  { "type": "minecraft:item", "name": "minecraft:dirt" }
                ]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:conditioned_alternatives").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:featured_alternatives").unwrap()),
            Some(&Identifier::parse("minecraft:dirt").unwrap())
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
    }

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }
}
