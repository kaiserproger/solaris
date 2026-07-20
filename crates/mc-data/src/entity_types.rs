//! Loader for the vanilla entity type registry slice of `registries.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;
use crate::entity_contract_26_1_2;

pub use crate::entity_contract_26_1_2::{
    DefaultAttributeTemplateIdentity, ENTITY_TYPE_COUNT, EntityArchetype, EntityBehaviorContract,
    EntityInstanceCategory, EntityInstanceContract, EntityTypeContract, EntityTypeFlags,
    MINECRAFT_VERSION, MetadataSchemaIdentity, MobCategory, PhysicalSimulationClass,
    ProjectileKind, SpawnDataCategory, VehiclePassengerCapabilities,
    ender_dragon_part_instance_contract_26_1_2, entity_type_contract_26_1_2_by_name,
    entity_type_contract_26_1_2_by_protocol_id,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityTypeReport {
    pub id: Identifier,
    pub protocol_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EntityTypeRegistryValidationError {
    #[error("entity type {name} has out-of-range protocol ID {protocol_id}")]
    ProtocolIdOutOfRange { protocol_id: u32, name: Identifier },
    #[error("duplicate entity type protocol ID {protocol_id}")]
    DuplicateProtocolId { protocol_id: u32 },
    #[error("duplicate entity type name {name}")]
    DuplicateName { name: Identifier },
    #[error("entity type protocol ID {protocol_id} is {actual_name}, expected {expected_name}")]
    NameMismatch {
        protocol_id: u32,
        expected_name: Identifier,
        actual_name: Identifier,
    },
    #[error("missing entity type protocol ID {protocol_id} ({expected_name})")]
    MissingProtocolId {
        protocol_id: u32,
        expected_name: Identifier,
    },
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
    pub spawn_dimensions_scale: f64,
}

impl EntityDimensions {
    #[must_use]
    pub const fn new(width: f64, height: f64, eye_height: Option<f64>) -> Self {
        Self {
            width,
            height,
            eye_height,
            spawn_dimensions_scale: 1.0,
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
    pub mob_category: Option<MobCategory>,
    pub dimensions: EntityDimensions,
    pub tracking_range: Option<u32>,
    pub update_interval: Option<u32>,
    pub flags: Option<EntityTypeFlags>,
    pub attributes: EntityAttributeFacts,
    pub loot_table: Option<Identifier>,
}

#[derive(Debug, Clone)]
pub struct EntityTypeRegistry {
    by_name: BTreeMap<Identifier, EntityTypeFacts>,
}

impl EntityTypeRegistry {
    /// Validates and constructs a complete Java Edition 26.1.2 registry.
    pub fn try_from_report_26_1_2(
        report: &[EntityTypeReport],
    ) -> Result<Self, EntityTypeRegistryValidationError> {
        validate_report_26_1_2(report)?;

        let by_name = report
            .iter()
            .map(|entry| {
                let contract = entity_contract_26_1_2::by_protocol_id(entry.protocol_id)
                    .expect("validated 26.1.2 report has an exact contract row");
                (
                    entry.id.clone(),
                    canonical_entity_type_facts(entry.id.clone(), entry.protocol_id, contract),
                )
            })
            .collect();
        Ok(Self { by_name })
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

fn validate_report_26_1_2(
    report: &[EntityTypeReport],
) -> Result<(), EntityTypeRegistryValidationError> {
    let mut seen_protocol_ids = [false; ENTITY_TYPE_COUNT];
    let mut seen_names = BTreeSet::new();

    for entry in report {
        let index = usize::try_from(entry.protocol_id).map_err(|_| {
            EntityTypeRegistryValidationError::ProtocolIdOutOfRange {
                protocol_id: entry.protocol_id,
                name: entry.id.clone(),
            }
        })?;
        if index >= ENTITY_TYPE_COUNT {
            return Err(EntityTypeRegistryValidationError::ProtocolIdOutOfRange {
                protocol_id: entry.protocol_id,
                name: entry.id.clone(),
            });
        }
        if seen_protocol_ids[index] {
            return Err(EntityTypeRegistryValidationError::DuplicateProtocolId {
                protocol_id: entry.protocol_id,
            });
        }
        if !seen_names.insert(&entry.id) {
            return Err(EntityTypeRegistryValidationError::DuplicateName {
                name: entry.id.clone(),
            });
        }

        let contract = entity_contract_26_1_2::by_protocol_id(entry.protocol_id)
            .expect("range-checked protocol ID has an exact contract row");
        if entry.id.as_str() != contract.name {
            return Err(EntityTypeRegistryValidationError::NameMismatch {
                protocol_id: entry.protocol_id,
                expected_name: contract_identifier(contract.name),
                actual_name: entry.id.clone(),
            });
        }
        seen_protocol_ids[index] = true;
    }

    for (index, seen) in seen_protocol_ids.into_iter().enumerate() {
        if !seen {
            let contract = entity_contract_26_1_2::by_protocol_id(index as u32)
                .expect("dense exact contract contains every in-range protocol ID");
            return Err(EntityTypeRegistryValidationError::MissingProtocolId {
                protocol_id: index as u32,
                expected_name: contract_identifier(contract.name),
            });
        }
    }

    Ok(())
}

fn contract_identifier(name: &'static str) -> Identifier {
    Identifier::parse(name).expect("exact entity contract names are valid identifiers")
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
    EntityTypeRegistry::try_from_report_26_1_2(&report)
        .expect("embedded required entity types match the exact 26.1.2 registry")
}

fn canonical_entity_type_facts(
    id: Identifier,
    protocol_id: u32,
    contract: EntityTypeContract,
) -> EntityTypeFacts {
    let category = entity_category_for_contract(id.as_str(), contract.category);
    let (attributes, loot_table) = independently_sourced_attributes_and_loot(&id);
    EntityTypeFacts {
        id,
        protocol_id,
        category,
        mob_category: Some(contract.category),
        dimensions: canonical_dimensions(contract),
        tracking_range: Some(u32::from(contract.tracking_range)),
        update_interval: Some(contract.update_interval),
        flags: Some(contract.flags),
        attributes,
        loot_table,
    }
}

fn canonical_dimensions(contract: EntityTypeContract) -> EntityDimensions {
    EntityDimensions {
        width: f64::from(contract.dimensions.width),
        height: f64::from(contract.dimensions.height),
        eye_height: Some(f64::from(contract.dimensions.eye_height)),
        spawn_dimensions_scale: f64::from(contract.dimensions.spawn_dimensions_scale),
    }
}

fn entity_category_for_contract(id: &str, mob_category: MobCategory) -> EntityCategory {
    match id {
        "minecraft:item" => EntityCategory::Item,
        "minecraft:experience_orb" => EntityCategory::Experience,
        _ => match mob_category {
            MobCategory::Monster => EntityCategory::Hostile,
            MobCategory::Creature | MobCategory::Ambient => EntityCategory::Passive,
            MobCategory::Axolotls
            | MobCategory::UndergroundWaterCreature
            | MobCategory::WaterCreature
            | MobCategory::WaterAmbient => EntityCategory::Water,
            MobCategory::Misc => EntityCategory::Other,
        },
    }
}

fn independently_sourced_attributes_and_loot(
    id: &Identifier,
) -> (EntityAttributeFacts, Option<Identifier>) {
    match id.as_str() {
        "minecraft:chicken" => (
            EntityAttributeFacts {
                max_health: Some(4.0),
                movement_speed: Some(0.25),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            static_id("minecraft:entities/chicken"),
        ),
        "minecraft:pig" => (
            EntityAttributeFacts {
                max_health: Some(10.0),
                movement_speed: Some(0.25),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            static_id("minecraft:entities/pig"),
        ),
        "minecraft:sheep" => (
            EntityAttributeFacts {
                max_health: Some(8.0),
                movement_speed: Some(0.23),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            static_id("minecraft:entities/sheep"),
        ),
        "minecraft:cow" => (
            EntityAttributeFacts {
                max_health: Some(10.0),
                movement_speed: Some(0.2),
                follow_range: Some(16.0),
                attack_damage: Some(0.0),
            },
            static_id("minecraft:entities/cow"),
        ),
        "minecraft:zombie" => (
            EntityAttributeFacts {
                max_health: Some(20.0),
                movement_speed: Some(0.23),
                follow_range: Some(35.0),
                attack_damage: Some(3.0),
            },
            static_id("minecraft:entities/zombie"),
        ),
        "minecraft:skeleton" => (
            EntityAttributeFacts {
                max_health: Some(20.0),
                movement_speed: Some(0.25),
                follow_range: Some(16.0),
                attack_damage: Some(2.0),
            },
            static_id("minecraft:entities/skeleton"),
        ),
        "minecraft:spider" => (
            EntityAttributeFacts {
                max_health: Some(16.0),
                movement_speed: Some(0.3),
                follow_range: Some(16.0),
                attack_damage: Some(2.0),
            },
            static_id("minecraft:entities/spider"),
        ),
        "minecraft:cod" | "minecraft:salmon" | "minecraft:tropical_fish" => (
            EntityAttributeFacts {
                max_health: Some(3.0),
                movement_speed: Some(0.7),
                follow_range: Some(8.0),
                attack_damage: Some(0.0),
            },
            static_id(&format!("minecraft:entities/{}", id.path())),
        ),
        _ => (EntityAttributeFacts::default(), None),
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
        let registry = solaris_required_entity_types();

        assert_eq!(
            registry.id_of(&Identifier::parse("minecraft:cow").unwrap()),
            Some(30)
        );
        assert_eq!(registry.len(), ENTITY_TYPE_COUNT);
    }

    #[test]
    fn registry_exposes_m32_entity_facts() {
        let registry = solaris_required_entity_types();

        let chicken = registry
            .facts_of(&Identifier::parse("minecraft:chicken").unwrap())
            .unwrap();
        assert_eq!(chicken.category, EntityCategory::Passive);
        assert_eq!(chicken.dimensions.width, f64::from(0.4_f32));
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
}
