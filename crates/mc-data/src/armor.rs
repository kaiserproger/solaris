use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

const BUILTIN_SURVIVAL_ARMOR: &str = include_str!("../data/survival_armor.json");

#[derive(Debug, Error)]
pub enum ArmorError {
    #[error("armor file {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid armor item identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("filesystem error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmorSlot {
    Head,
    Chest,
    Legs,
    Feet,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct ArmorEntry {
    pub slot: ArmorSlot,
    pub armor: f32,
    pub toughness: f32,
    pub max_damage: i32,
}

#[derive(Debug, Clone, Default)]
pub struct ArmorTable {
    items: BTreeMap<Identifier, ArmorEntry>,
}

impl ArmorTable {
    #[must_use]
    pub fn entry(&self, item: &Identifier) -> Option<&ArmorEntry> {
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
struct RawArmorTable {
    items: BTreeMap<String, ArmorEntry>,
}

#[must_use]
pub fn builtin() -> &'static ArmorTable {
    static BUILTIN: OnceLock<ArmorTable> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        from_str(
            BUILTIN_SURVIVAL_ARMOR,
            Path::new("crates/mc-data/data/survival_armor.json"),
        )
        .expect("built-in Solaris survival armor JSON is valid")
    })
}

pub fn load(path: impl AsRef<Path>) -> Result<ArmorTable, ArmorError> {
    let path = path.as_ref();
    let bytes = crate::sidecar::read_string(path).map_err(|source| ArmorError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    from_str(&bytes, path)
}

fn from_str(raw: &str, path: &Path) -> Result<ArmorTable, ArmorError> {
    let raw: RawArmorTable = serde_json::from_str(raw).map_err(|source| ArmorError::Malformed {
        path: path.to_path_buf(),
        source,
    })?;
    let items = raw
        .items
        .into_iter()
        .map(|(item, entry)| {
            let item =
                Identifier::parse(item.clone()).map_err(|_| ArmorError::InvalidIdentifier {
                    path: path.to_path_buf(),
                    value: item,
                })?;
            Ok((item, entry))
        })
        .collect::<Result<_, _>>()?;
    Ok(ArmorTable { items })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_survival_armor_loads_from_repo_json() {
        let armor = builtin();

        assert_eq!(armor.len(), 29);
        assert_eq!(
            armor.entry(&Identifier::parse("minecraft:iron_chestplate").unwrap()),
            Some(&ArmorEntry {
                slot: ArmorSlot::Chest,
                armor: 6.0,
                toughness: 0.0,
                max_damage: 240,
            })
        );
        assert_eq!(
            armor.entry(&Identifier::parse("minecraft:diamond_leggings").unwrap()),
            Some(&ArmorEntry {
                slot: ArmorSlot::Legs,
                armor: 6.0,
                toughness: 2.0,
                max_damage: 495,
            })
        );
    }
}
