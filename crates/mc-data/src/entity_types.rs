//! Loader for the vanilla entity type registry slice of `registries.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

const REQUIRED_REGISTRIES: &str = include_str!("../data/required_registries.json");

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityCategory {
    Passive,
    Hostile,
    Water,
    Item,
    Experience,
    Other,
}

impl EntityCategory {
    #[must_use]
    pub const fn is_hostile(self) -> bool {
        matches!(self, Self::Hostile)
    }

    #[must_use]
    pub const fn is_living(self) -> bool {
        matches!(self, Self::Passive | Self::Hostile | Self::Water)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityDimensions {
    pub width: f64,
    pub height: f64,
    pub eye_height: Option<f64>,
}

impl EntityDimensions {
    #[must_use]
    pub const fn new(width: f64, height: f64, eye_height: Option<f64>) -> Self {
        Self {
            width,
            height,
            eye_height,
        }
    }

    #[must_use]
    pub const fn half_width(self) -> f64 {
        self.width / 2.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EntityAttributeFacts {
    pub max_health: Option<f64>,
    pub movement_speed: Option<f64>,
    pub follow_range: Option<f64>,
    pub attack_damage: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityTypeFacts {
    pub id: Identifier,
    pub protocol_id: u32,
    pub category: EntityCategory,
    pub dimensions: EntityDimensions,
    pub tracking_range: Option<u32>,
    pub attributes: EntityAttributeFacts,
    pub loot_table: Option<Identifier>,
}

#[derive(Debug, Clone, Default)]
pub struct EntityTypeRegistry {
    by_name: BTreeMap<Identifier, EntityTypeFacts>,
}

impl EntityTypeRegistry {
    #[must_use]
    pub fn from_report(report: &[EntityTypeReport]) -> Self {
        let by_name = report
            .iter()
            .map(|entry| {
                (
                    entry.id.clone(),
                    fallback_entity_type_facts(entry.id.clone(), entry.protocol_id),
                )
            })
            .collect();
        Self { by_name }
    }

    #[must_use]
    pub fn id_of(&self, name: &Identifier) -> Option<u32> {
        self.by_name.get(name).map(|facts| facts.protocol_id)
    }

    #[must_use]
    pub fn facts_of(&self, name: &Identifier) -> Option<&EntityTypeFacts> {
        self.by_name.get(name)
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

/// Repo-owned entity type slice used when `registries.json` is absent.
#[must_use]
pub fn solaris_required_entity_types() -> EntityTypeRegistry {
    let raw: RawRegistries = serde_json::from_str(REQUIRED_REGISTRIES)
        .expect("embedded required registries JSON is valid");
    let entity_types = raw
        .registries
        .get("minecraft:entity_type")
        .expect("embedded required registries JSON contains minecraft:entity_type");
    let report: Vec<_> = entity_types
        .entries
        .iter()
        .map(|(name, body)| EntityTypeReport {
            id: Identifier::parse(name.clone()).expect("embedded required entity type id is valid"),
            protocol_id: body.protocol_id,
        })
        .collect();
    EntityTypeRegistry::from_report(&report)
}

#[must_use]
pub fn fallback_entity_category(id: &str) -> EntityCategory {
    match id {
        "minecraft:chicken" | "minecraft:pig" | "minecraft:sheep" | "minecraft:cow" => {
            EntityCategory::Passive
        }
        "minecraft:zombie" | "minecraft:skeleton" | "minecraft:spider" => EntityCategory::Hostile,
        "minecraft:cod" | "minecraft:salmon" | "minecraft:tropical_fish" => EntityCategory::Water,
        "minecraft:item" => EntityCategory::Item,
        "minecraft:experience_orb" | "minecraft:xp_orb" => EntityCategory::Experience,
        _ => EntityCategory::Other,
    }
}

#[must_use]
pub fn fallback_entity_dimensions(id: &str, is_baby: bool) -> Option<EntityDimensions> {
    match (id, is_baby) {
        ("minecraft:chicken", false) => Some(EntityDimensions::new(0.4, 0.7, Some(0.644))),
        ("minecraft:chicken", true) => Some(EntityDimensions::new(0.3, 0.4, Some(0.28))),
        ("minecraft:cow", false) => Some(EntityDimensions::new(0.9, 1.4, Some(1.3))),
        ("minecraft:cow", true) => Some(EntityDimensions::new(0.45, 0.7, Some(0.665))),
        ("minecraft:pig", false) => Some(EntityDimensions::new(0.9, 0.9, Some(0.765))),
        ("minecraft:pig", true) => Some(EntityDimensions::new(0.45, 0.45, Some(0.3825))),
        ("minecraft:sheep", false) => Some(EntityDimensions::new(0.9, 1.3, Some(1.235))),
        ("minecraft:sheep", true) => Some(EntityDimensions::new(0.45, 0.65, Some(0.6175))),
        _ => None,
    }
}

#[must_use]
pub fn fallback_entity_type_facts(id: Identifier, protocol_id: u32) -> EntityTypeFacts {
    let category = fallback_entity_category(id.as_str());
    let (dimensions, tracking_range, attributes, loot_table) = match id.as_str() {
        "minecraft:chicken" => (
            EntityDimensions::new(0.4, 0.7, Some(0.644)),
            Some(10),
            EntityAttributeFacts {
                max_health: Some(4.0),
                movement_speed: Some(0.25),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            static_id("minecraft:entities/chicken"),
        ),
        "minecraft:pig" => (
            EntityDimensions::new(0.9, 0.9, Some(0.765)),
            Some(10),
            EntityAttributeFacts {
                max_health: Some(10.0),
                movement_speed: Some(0.25),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            static_id("minecraft:entities/pig"),
        ),
        "minecraft:sheep" => (
            EntityDimensions::new(0.9, 1.3, Some(1.235)),
            Some(10),
            EntityAttributeFacts {
                max_health: Some(8.0),
                movement_speed: Some(0.23),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            static_id("minecraft:entities/sheep"),
        ),
        "minecraft:cow" => (
            EntityDimensions::new(0.9, 1.4, Some(1.3)),
            Some(10),
            EntityAttributeFacts {
                max_health: Some(10.0),
                movement_speed: Some(0.2),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            static_id("minecraft:entities/cow"),
        ),
        "minecraft:zombie" => (
            EntityDimensions::new(0.6, 1.95, Some(1.74)),
            Some(8),
            EntityAttributeFacts {
                max_health: Some(20.0),
                movement_speed: Some(0.23),
                follow_range: Some(35.0),
                attack_damage: Some(3.0),
            },
            static_id("minecraft:entities/zombie"),
        ),
        "minecraft:skeleton" => (
            EntityDimensions::new(0.6, 1.99, Some(1.74)),
            Some(8),
            EntityAttributeFacts {
                max_health: Some(20.0),
                movement_speed: Some(0.25),
                follow_range: Some(16.0),
                attack_damage: Some(2.0),
            },
            static_id("minecraft:entities/skeleton"),
        ),
        "minecraft:spider" => (
            EntityDimensions::new(1.4, 0.9, Some(0.65)),
            Some(8),
            EntityAttributeFacts {
                max_health: Some(16.0),
                movement_speed: Some(0.3),
                follow_range: Some(16.0),
                attack_damage: Some(2.0),
            },
            static_id("minecraft:entities/spider"),
        ),
        "minecraft:cod" | "minecraft:salmon" | "minecraft:tropical_fish" => (
            EntityDimensions::new(0.5, 0.3, Some(0.195)),
            Some(4),
            EntityAttributeFacts {
                max_health: Some(3.0),
                movement_speed: Some(0.7),
                follow_range: Some(8.0),
                attack_damage: Some(0.0),
            },
            static_id(&format!("minecraft:entities/{}", id.path())),
        ),
        "minecraft:item" => (
            EntityDimensions::new(0.25, 0.25, None),
            Some(6),
            EntityAttributeFacts::default(),
            None,
        ),
        "minecraft:experience_orb" | "minecraft:xp_orb" => (
            EntityDimensions::new(0.5, 0.5, None),
            Some(6),
            EntityAttributeFacts::default(),
            None,
        ),
        "minecraft:falling_block" => (
            EntityDimensions::new(0.98, 0.98, None),
            Some(10),
            EntityAttributeFacts::default(),
            None,
        ),
        "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:tipped_arrow" => (
            EntityDimensions::new(0.5, 0.5, None),
            Some(4),
            EntityAttributeFacts::default(),
            None,
        ),
        _ => (
            EntityDimensions::new(0.9, 1.4, Some(1.3)),
            None,
            EntityAttributeFacts {
                max_health: Some(20.0),
                movement_speed: Some(0.25),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            None,
        ),
    };
    EntityTypeFacts {
        id,
        protocol_id,
        category,
        dimensions,
        tracking_range,
        attributes,
        loot_table,
    }
}

fn static_id(value: &str) -> Option<Identifier> {
    Identifier::parse(value).ok()
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

    #[test]
    fn registry_exposes_m32_entity_facts() {
        let registry = EntityTypeRegistry::from_report(&[
            EntityTypeReport {
                id: Identifier::parse("minecraft:chicken").unwrap(),
                protocol_id: 10,
            },
            EntityTypeReport {
                id: Identifier::parse("minecraft:skeleton").unwrap(),
                protocol_id: 20,
            },
            EntityTypeReport {
                id: Identifier::parse("minecraft:item").unwrap(),
                protocol_id: 30,
            },
        ]);

        let chicken = registry
            .facts_of(&Identifier::parse("minecraft:chicken").unwrap())
            .unwrap();
        assert_eq!(chicken.category, EntityCategory::Passive);
        assert_eq!(chicken.dimensions.width, 0.4);
        assert_eq!(chicken.attributes.max_health, Some(4.0));
        assert_eq!(
            chicken.loot_table.as_ref().map(Identifier::as_str),
            Some("minecraft:entities/chicken")
        );

        let skeleton = registry
            .facts_of(&Identifier::parse("minecraft:skeleton").unwrap())
            .unwrap();
        assert!(skeleton.category.is_hostile());
        assert_eq!(skeleton.attributes.attack_damage, Some(2.0));

        let item = registry
            .facts_of(&Identifier::parse("minecraft:item").unwrap())
            .unwrap();
        assert_eq!(item.category, EntityCategory::Item);
        assert_eq!(item.dimensions.height, 0.25);
    }

    #[test]
    fn fallback_category_matches_full_facts_without_identifier_input() {
        for name in [
            "minecraft:cow",
            "minecraft:zombie",
            "minecraft:skeleton",
            "minecraft:spider",
            "minecraft:cod",
            "minecraft:item",
            "minecraft:experience_orb",
            "minecraft:arrow",
            "minecraft:unknown",
        ] {
            let facts = fallback_entity_type_facts(Identifier::parse(name).unwrap(), 0);
            assert_eq!(fallback_entity_category(name), facts.category, "{name}");
        }
    }

    #[test]
    fn livestock_dimensions_match_vanilla_for_adults_and_babies() {
        let cases = [
            (
                "minecraft:chicken",
                false,
                EntityDimensions::new(0.4, 0.7, Some(0.644)),
            ),
            (
                "minecraft:chicken",
                true,
                EntityDimensions::new(0.3, 0.4, Some(0.28)),
            ),
            (
                "minecraft:cow",
                false,
                EntityDimensions::new(0.9, 1.4, Some(1.3)),
            ),
            (
                "minecraft:cow",
                true,
                EntityDimensions::new(0.45, 0.7, Some(0.665)),
            ),
            (
                "minecraft:pig",
                false,
                EntityDimensions::new(0.9, 0.9, Some(0.765)),
            ),
            (
                "minecraft:pig",
                true,
                EntityDimensions::new(0.45, 0.45, Some(0.3825)),
            ),
            (
                "minecraft:sheep",
                false,
                EntityDimensions::new(0.9, 1.3, Some(1.235)),
            ),
            (
                "minecraft:sheep",
                true,
                EntityDimensions::new(0.45, 0.65, Some(0.6175)),
            ),
        ];

        for (id, is_baby, expected) in cases {
            assert_eq!(
                fallback_entity_dimensions(id, is_baby),
                Some(expected),
                "{id}"
            );
            if !is_baby {
                let facts = fallback_entity_type_facts(Identifier::parse(id).unwrap(), 0);
                assert_eq!(facts.dimensions, expected, "adult facts for {id}");
            }
        }
    }
}
