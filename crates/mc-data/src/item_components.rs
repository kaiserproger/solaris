use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;
use crate::food::FoodEntry;

const DEFAULT_CONSUME_SECONDS: f32 = 1.6;

#[derive(Debug, Error)]
pub enum ItemComponentsError {
    #[error("item component reports io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("item component report parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid item component report filename at {path}")]
    InvalidPath { path: PathBuf },
    #[error("invalid item identifier {value:?} in component report {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
}

#[derive(Debug, Clone, Default)]
pub struct ItemFactsTable {
    items: BTreeMap<Identifier, ItemFacts>,
}

impl ItemFactsTable {
    #[must_use]
    pub fn get(&self, item: &Identifier) -> Option<&ItemFacts> {
        self.items.get(item)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemFacts {
    pub max_stack_size: Option<u32>,
    pub max_damage: Option<u32>,
    pub food: Option<FoodEntry>,
    pub use_duration_ticks: Option<u32>,
    pub use_action: Option<UseAction>,
    pub tool: Option<ToolFacts>,
    pub equippable_slot: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseAction {
    Eat,
    Drink,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolFacts {
    pub default_mining_speed: Option<f32>,
    pub damage_per_block: Option<u32>,
}

pub fn load_item_facts(
    report_dir: impl AsRef<Path>,
) -> Result<ItemFactsTable, ItemComponentsError> {
    let report_dir = report_dir.as_ref();
    if !report_dir.is_dir() {
        return Ok(ItemFactsTable::default());
    }

    let mut paths = Vec::new();
    collect_json_files(report_dir, &mut paths)?;
    paths.sort();

    let mut items = BTreeMap::new();
    for path in paths {
        let id = id_from_path(report_dir, &path)?;
        let facts = load_one(&path)?;
        items.insert(id, facts);
    }
    Ok(ItemFactsTable { items })
}

fn collect_json_files(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), ItemComponentsError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ItemComponentsError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| ItemComponentsError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let ty = entry
            .file_type()
            .map_err(|source| ItemComponentsError::Io {
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

fn id_from_path(root: &Path, path: &Path) -> Result<Identifier, ItemComponentsError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| ItemComponentsError::InvalidPath {
            path: path.to_path_buf(),
        })?
        .with_extension("");
    let mut value = String::from("minecraft:");
    for component in rel.components() {
        if !value.ends_with(':') {
            value.push('/');
        }
        value.push_str(component.as_os_str().to_string_lossy().as_ref());
    }
    Identifier::parse(value.clone()).map_err(|_| ItemComponentsError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

fn load_one(path: &Path) -> Result<ItemFacts, ItemComponentsError> {
    let bytes = std::fs::read_to_string(path).map_err(|source| ItemComponentsError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let raw: RawItemComponents =
        serde_json::from_str(&bytes).map_err(|source| ItemComponentsError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(raw.into_facts())
}

#[derive(Deserialize)]
struct RawItemComponents {
    components: RawComponents,
}

#[derive(Default, Deserialize)]
struct RawComponents {
    #[serde(rename = "minecraft:max_stack_size")]
    max_stack_size: Option<u32>,
    #[serde(rename = "minecraft:max_damage")]
    max_damage: Option<u32>,
    #[serde(rename = "minecraft:food")]
    food: Option<RawFood>,
    #[serde(rename = "minecraft:consumable")]
    consumable: Option<RawConsumable>,
    #[serde(rename = "minecraft:tool")]
    tool: Option<RawTool>,
    #[serde(rename = "minecraft:equippable")]
    equippable: Option<RawEquippable>,
}

impl RawItemComponents {
    fn into_facts(self) -> ItemFacts {
        let RawComponents {
            max_stack_size,
            max_damage,
            food,
            consumable,
            tool,
            equippable,
        } = self.components;
        let use_duration_ticks = consumable.as_ref().map(|raw| {
            ((raw.consume_seconds.unwrap_or(DEFAULT_CONSUME_SECONDS) * 20.0).round()) as u32
        });
        let use_action = consumable
            .as_ref()
            .map(|raw| match raw.animation.as_deref() {
                None | Some("eat") | Some("minecraft:eat") => UseAction::Eat,
                Some("drink") | Some("minecraft:drink") => UseAction::Drink,
                Some(_) => UseAction::Other,
            });
        ItemFacts {
            max_stack_size,
            max_damage,
            food: food.map(|raw| FoodEntry {
                food: raw.nutrition,
                saturation: raw.saturation,
            }),
            use_duration_ticks,
            use_action,
            tool: tool.map(|raw| ToolFacts {
                default_mining_speed: raw.default_mining_speed,
                damage_per_block: raw.damage_per_block,
            }),
            equippable_slot: equippable.map(|raw| raw.slot),
        }
    }
}

#[derive(Deserialize)]
struct RawFood {
    nutrition: i32,
    saturation: f32,
}

#[derive(Default, Deserialize)]
struct RawConsumable {
    consume_seconds: Option<f32>,
    animation: Option<String>,
}

#[derive(Deserialize)]
struct RawTool {
    default_mining_speed: Option<f32>,
    damage_per_block: Option<u32>,
}

#[derive(Deserialize)]
struct RawEquippable {
    slot: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn loads_component_report_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("apple.json"),
            r#"{
              "components": {
                "minecraft:max_stack_size": 64,
                "minecraft:food": { "nutrition": 4, "saturation": 2.4 },
                "minecraft:consumable": {}
              }
            }"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("iron_helmet.json"),
            r#"{
              "components": {
                "minecraft:max_damage": 165,
                "minecraft:max_stack_size": 1,
                "minecraft:equippable": { "slot": "head" }
              }
            }"#,
        )
        .unwrap();

        let facts = load_item_facts(tmp.path()).unwrap();

        let apple = facts
            .get(&Identifier::parse("minecraft:apple").unwrap())
            .unwrap();
        assert_eq!(apple.max_stack_size, Some(64));
        assert_eq!(
            apple.food,
            Some(FoodEntry {
                food: 4,
                saturation: 2.4
            })
        );
        assert_eq!(apple.use_duration_ticks, Some(32));
        assert_eq!(apple.use_action, Some(UseAction::Eat));

        let helmet = facts
            .get(&Identifier::parse("minecraft:iron_helmet").unwrap())
            .unwrap();
        assert_eq!(helmet.max_damage, Some(165));
        assert_eq!(helmet.max_stack_size, Some(1));
        assert_eq!(helmet.equippable_slot.as_deref(), Some("head"));
    }

    #[test]
    fn missing_component_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let facts = load_item_facts(tmp.path().join("missing")).unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn loads_real_apple_and_bread_components_when_present() {
        let path = workspace_path("data/vanilla/reports/minecraft/components/item");
        if !path.is_dir() {
            eprintln!("skipping: {} not present", path.display());
            return;
        }

        let facts = load_item_facts(path).unwrap();

        let apple = facts
            .get(&Identifier::parse("minecraft:apple").unwrap())
            .unwrap();
        assert_eq!(apple.max_stack_size, Some(64));
        assert_eq!(
            apple.food,
            Some(FoodEntry {
                food: 4,
                saturation: 2.4
            })
        );
        assert_eq!(apple.use_duration_ticks, Some(32));
        assert_eq!(apple.use_action, Some(UseAction::Eat));

        let bread = facts
            .get(&Identifier::parse("minecraft:bread").unwrap())
            .unwrap();
        assert_eq!(bread.max_stack_size, Some(64));
        assert_eq!(
            bread.food,
            Some(FoodEntry {
                food: 5,
                saturation: 6.0
            })
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
