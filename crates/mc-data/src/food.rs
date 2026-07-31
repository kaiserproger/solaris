use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
            food.entry(&Identifier::parse("minecraft:dirt").unwrap()),
            None
        );
    }
}
