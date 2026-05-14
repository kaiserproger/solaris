//! Loader for the vanilla entity type registry slice of `registries.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

#[derive(Debug, Error)]
pub enum EntityTypesReportError {
    #[error("registries.json not found at {0}")]
    Missing(PathBuf),
    #[error("registries.json io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("registries.json parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("registries.json does not contain minecraft:entity_type")]
    MissingEntityTypeRegistry,
    #[error("invalid entity type identifier {0:?} in registries.json")]
    InvalidIdentifier(String),
}

pub fn load_entity_types_report(
    path: impl AsRef<Path>,
) -> Result<Vec<EntityTypeReport>, EntityTypesReportError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(EntityTypesReportError::Missing(path.to_path_buf()));
    }
    let bytes = std::fs::read(path)?;
    let raw: RawRegistries = serde_json::from_slice(&bytes)?;
    let entity_types = raw
        .registries
        .get("minecraft:entity_type")
        .ok_or(EntityTypesReportError::MissingEntityTypeRegistry)?;
    entity_types
        .entries
        .iter()
        .map(|(name, body)| {
            let id = Identifier::parse(name.clone())
                .map_err(|_| EntityTypesReportError::InvalidIdentifier(name.clone()))?;
            Ok(EntityTypeReport {
                id,
                protocol_id: body.protocol_id,
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct EntityTypeReport {
    pub id: Identifier,
    pub protocol_id: u32,
}

#[derive(Debug, Clone, Default)]
pub struct EntityTypeRegistry {
    by_name: BTreeMap<Identifier, u32>,
}

impl EntityTypeRegistry {
    #[must_use]
    pub fn from_report(report: &[EntityTypeReport]) -> Self {
        let by_name = report
            .iter()
            .map(|entry| (entry.id.clone(), entry.protocol_id))
            .collect();
        Self { by_name }
    }

    #[must_use]
    pub fn id_of(&self, name: &Identifier) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

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

    #[test]
    fn registry_resolves_entity_type_ids() {
        let registry = EntityTypeRegistry::from_report(&[
            EntityTypeReport {
                id: Identifier::parse("minecraft:cow").unwrap(),
                protocol_id: 30,
            },
            EntityTypeReport {
                id: Identifier::parse("minecraft:zombie").unwrap(),
                protocol_id: 147,
            },
        ]);

        assert_eq!(
            registry.id_of(&Identifier::parse("minecraft:cow").unwrap()),
            Some(30)
        );
        assert_eq!(registry.len(), 2);
    }
}
