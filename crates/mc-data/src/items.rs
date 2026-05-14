//! Loader for the vanilla item registry slice of `registries.json`.
//!
//! `<data_dir>/reports/registries.json` exposes every registry's
//! `{ default, entries: { name -> { protocol_id } }, protocol_id }`
//! shape. We only consume `minecraft:item` here; other callers can
//! reuse the same file for their own registries when they need them.
//!
//! The `ItemRegistry` type is plain data: a `BTreeMap<Identifier, u32>`
//! plus the reverse lookup. No semantic information about the items
//! (max stack size, components, …) is stored — those land with the
//! wider data-pack work in M9+.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

#[derive(Debug, Error)]
pub enum ItemsReportError {
    #[error("registries.json not found at {0}")]
    Missing(PathBuf),
    #[error("registries.json io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("registries.json parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("registries.json does not contain minecraft:item")]
    MissingItemRegistry,
    #[error("invalid item identifier {0:?} in registries.json")]
    InvalidIdentifier(String),
}

/// Read `registries.json` and pull out the `minecraft:item` slice.
pub fn load_items_report(path: impl AsRef<Path>) -> Result<Vec<ItemReport>, ItemsReportError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(ItemsReportError::Missing(path.to_path_buf()));
    }
    let bytes = std::fs::read(path)?;
    let raw: RawRegistries = serde_json::from_slice(&bytes)?;
    let items = raw
        .registries
        .get("minecraft:item")
        .ok_or(ItemsReportError::MissingItemRegistry)?;
    items
        .entries
        .iter()
        .map(|(name, body)| {
            let id = Identifier::parse(name.clone())
                .map_err(|_| ItemsReportError::InvalidIdentifier(name.clone()))?;
            Ok(ItemReport {
                id,
                protocol_id: body.protocol_id,
            })
        })
        .collect()
}

/// One entry of the item registry.
#[derive(Debug, Clone)]
pub struct ItemReport {
    pub id: Identifier,
    pub protocol_id: u32,
}

/// Typed item registry: bidirectional name↔id lookup. Built once at
/// startup from [`load_items_report`].
#[derive(Debug, Clone, Default)]
pub struct ItemRegistry {
    by_name: BTreeMap<Identifier, u32>,
    by_id: BTreeMap<u32, Identifier>,
}

impl ItemRegistry {
    #[must_use]
    pub fn from_report(report: &[ItemReport]) -> Self {
        let mut by_name = BTreeMap::new();
        let mut by_id = BTreeMap::new();
        for r in report {
            by_name.insert(r.id.clone(), r.protocol_id);
            by_id.insert(r.protocol_id, r.id.clone());
        }
        Self { by_name, by_id }
    }

    #[must_use]
    pub fn id_of(&self, name: &Identifier) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    #[must_use]
    pub fn name_of(&self, id: u32) -> Option<&Identifier> {
        self.by_id.get(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Identifier, u32)> {
        self.by_name.iter().map(|(name, id)| (name, *id))
    }
}

// Raw deserialisation shape — keep separate so the public ItemReport
// stays plain and serde-free for downstream consumers.

#[derive(Deserialize)]
struct RawRegistries {
    #[serde(flatten)]
    registries: BTreeMap<String, RawRegistry>,
}

#[derive(Deserialize)]
struct RawRegistry {
    entries: BTreeMap<String, RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    protocol_id: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    #[test]
    fn loads_real_item_registry_when_present() {
        let path = workspace_path("data/vanilla/reports/registries.json");
        if !path.is_file() {
            eprintln!("skipping: {} not present", path.display());
            return;
        }
        let report = load_items_report(&path).unwrap();
        assert!(report.len() > 1000, "vanilla has > 1k item kinds");
        let reg = ItemRegistry::from_report(&report);
        // Spot check a few items whose protocol ids should be stable
        // across patch releases of the same major version.
        let air = Identifier::parse("minecraft:air").unwrap();
        let stone = Identifier::parse("minecraft:stone").unwrap();
        let dirt = Identifier::parse("minecraft:dirt").unwrap();
        let oak = Identifier::parse("minecraft:oak_planks").unwrap();
        let torch = Identifier::parse("minecraft:torch").unwrap();
        assert_eq!(reg.id_of(&air), Some(0));
        assert!(reg.id_of(&stone).is_some());
        assert!(reg.id_of(&dirt).is_some());
        assert!(reg.id_of(&oak).is_some());
        assert!(reg.id_of(&torch).is_some());
        // Round-trip.
        let stone_id = reg.id_of(&stone).unwrap();
        assert_eq!(reg.name_of(stone_id), Some(&stone));
    }

    #[test]
    fn iter_yields_names_and_protocol_ids() {
        let report = [
            ItemReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                protocol_id: 0,
            },
            ItemReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 1,
            },
        ];
        let reg = ItemRegistry::from_report(&report);

        let entries: Vec<_> = reg
            .iter()
            .map(|(name, id)| (name.as_str().to_string(), id))
            .collect();

        assert_eq!(
            entries,
            vec![
                ("minecraft:air".to_string(), 0),
                ("minecraft:stone".to_string(), 1),
            ]
        );
    }
}
