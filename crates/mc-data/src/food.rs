use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

const BUILTIN_SURVIVAL_FOOD: &str = include_str!("../data/survival_food.json");

#[derive(Debug, Error)]
pub enum FoodError {
    #[error("food file {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid food item identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("filesystem error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub const DEFAULT_USE_DURATION: Duration = Duration::from_millis(1_600);

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct FoodEntry {
    pub food: i32,
    pub saturation: f32,
}

#[derive(Debug, Clone, Default)]
pub struct FoodTable {
    items: BTreeMap<Identifier, FoodEntry>,
}

impl FoodTable {
    #[must_use]
    pub fn entry(&self, item: &Identifier) -> Option<&FoodEntry> {
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

#[derive(Deserialize)]
struct RawFoodTable {
    items: BTreeMap<String, FoodEntry>,
}

#[must_use]
pub fn builtin() -> &'static FoodTable {
    static BUILTIN: OnceLock<FoodTable> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        from_str(
            BUILTIN_SURVIVAL_FOOD,
            Path::new("crates/mc-data/data/survival_food.json"),
        )
        .expect("built-in Solaris survival food JSON is valid")
    })
}

#[must_use]
pub fn rule_for_item(
    item_facts: &crate::item_components::ItemFactsTable,
    item: &Identifier,
    default_use_duration: Duration,
) -> Option<(FoodEntry, Duration)> {
    if let Some(facts) = item_facts.get(item)
        && let Some(food) = facts.food
    {
        let duration = facts
            .use_duration_ticks
            .map(|ticks| Duration::from_millis(u64::from(ticks) * 50))
            .unwrap_or(default_use_duration);
        return Some((food, duration));
    }

    builtin()
        .entry(item)
        .copied()
        .map(|food| (food, default_use_duration))
}

pub fn load(path: impl AsRef<Path>) -> Result<FoodTable, FoodError> {
    let path = path.as_ref();
    let bytes = crate::sidecar::read_string(path).map_err(|source| FoodError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    from_str(&bytes, path)
}

fn from_str(raw: &str, path: &Path) -> Result<FoodTable, FoodError> {
    let raw: RawFoodTable = serde_json::from_str(raw).map_err(|source| FoodError::Malformed {
        path: path.to_path_buf(),
        source,
    })?;
    let items = raw
        .items
        .into_iter()
        .map(|(item, entry)| {
            let item =
                Identifier::parse(item.clone()).map_err(|_| FoodError::InvalidIdentifier {
                    path: path.to_path_buf(),
                    value: item,
                })?;
            Ok((item, entry))
        })
        .collect::<Result<_, _>>()?;
    Ok(FoodTable { items })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_facts_override_builtin_food_and_use_duration() {
        let item = Identifier::parse("minecraft:apple").unwrap();
        let facts = crate::item_components::ItemFactsTable::from_entries([(
            item.clone(),
            crate::item_components::ItemFacts {
                food: Some(FoodEntry {
                    food: 7,
                    saturation: 3.5,
                }),
                use_duration_ticks: Some(10),
                ..Default::default()
            },
        )]);

        assert_eq!(
            rule_for_item(&facts, &item, DEFAULT_USE_DURATION),
            Some((
                FoodEntry {
                    food: 7,
                    saturation: 3.5,
                },
                Duration::from_millis(500),
            ))
        );
    }

    #[test]
    fn builtin_food_is_used_when_item_facts_are_absent() {
        let item = Identifier::parse("minecraft:apple").unwrap();
        let duration = DEFAULT_USE_DURATION;
        assert_eq!(
            rule_for_item(
                &crate::item_components::ItemFactsTable::default(),
                &item,
                duration
            ),
            builtin().entry(&item).copied().map(|food| (food, duration))
        );
    }

    #[test]
    fn builtin_survival_food_loads_from_repo_json() {
        let food = builtin();

        assert_eq!(food.len(), 2);
        assert_eq!(
            food.entry(&Identifier::parse("minecraft:apple").unwrap()),
            Some(&FoodEntry {
                food: 4,
                saturation: 2.4,
            })
        );
        assert_eq!(
            food.entry(&Identifier::parse("minecraft:bread").unwrap()),
            Some(&FoodEntry {
                food: 5,
                saturation: 6.0,
            })
        );
        assert_eq!(
            food.entry(&Identifier::parse("minecraft:dirt").unwrap()),
            None
        );
    }
}
