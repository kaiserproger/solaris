use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::{Identifier, read_json_file, visit_json_files};

const REQUIRED_DAMAGE_TYPES: &str = include_str!("../data/required_damage_types.json");

#[derive(Debug, Error)]
pub enum DamageTypesError {
    #[error("damage type directory not found at {0}")]
    Missing(PathBuf),
    #[error("damage type file io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("damage type file parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid damage type identifier {value:?} at {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageCategory {
    Generic,
    PlayerAttack,
    MobAttack,
    Projectile,
    Fall,
    Fire,
    Drowning,
    Suffocation,
    Starvation,
    Explosion,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DamageTypeFacts {
    pub id: Identifier,
    pub category: DamageCategory,
    pub message_id: String,
    pub scaling: String,
    pub exhaustion: f32,
    pub effects: Option<String>,
    pub death_message_type: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DamageTypeTable {
    entries: BTreeMap<Identifier, DamageTypeFacts>,
}

impl DamageTypeTable {
    #[must_use]
    pub fn from_entries(entries: impl IntoIterator<Item = DamageTypeFacts>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|facts| (facts.id.clone(), facts))
                .collect(),
        }
    }

    #[must_use]
    pub fn with_solaris_fallbacks() -> Self {
        let raw: BTreeMap<String, RawDamageType> = serde_json::from_str(REQUIRED_DAMAGE_TYPES)
            .expect("embedded required damage type JSON is valid");
        Self::from_entries(raw.into_iter().map(|(id, raw)| {
            raw.into_facts(
                Identifier::parse(id).expect("embedded required damage type id is valid"),
            )
        }))
    }

    #[must_use]
    pub fn get(&self, id: &Identifier) -> Option<&DamageTypeFacts> {
        self.entries.get(id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &DamageTypeFacts> {
        self.entries.values()
    }
}

pub fn load_damage_type_facts(
    damage_type_dir: impl AsRef<Path>,
) -> Result<DamageTypeTable, DamageTypesError> {
    let damage_type_dir = damage_type_dir.as_ref();
    if !damage_type_dir.is_dir() {
        return Err(DamageTypesError::Missing(damage_type_dir.to_path_buf()));
    }

    let mut paths = Vec::new();
    visit_json_files(
        damage_type_dir,
        &mut |path| {
            paths.push(path);
            Ok(())
        },
        &|path, source| DamageTypesError::Io { path, source },
    )?;
    paths.sort();

    let mut entries = Vec::new();
    for path in paths {
        let id = id_from_path(damage_type_dir, &path)?;
        let raw = load_one(&path)?;
        entries.push(raw.into_facts(id));
    }
    Ok(DamageTypeTable::from_entries(entries))
}

fn id_from_path(root: &Path, path: &Path) -> Result<Identifier, DamageTypesError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| DamageTypesError::InvalidIdentifier {
            path: path.to_path_buf(),
            value: path.display().to_string(),
        })?
        .with_extension("");
    let mut value = String::from("minecraft:");
    for component in rel.components() {
        if !value.ends_with(':') {
            value.push('/');
        }
        value.push_str(component.as_os_str().to_string_lossy().as_ref());
    }
    Identifier::parse(value.clone()).map_err(|_| DamageTypesError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

fn load_one(path: &Path) -> Result<RawDamageType, DamageTypesError> {
    read_json_file(
        path,
        &|path, source| DamageTypesError::Io { path, source },
        &|path, source| DamageTypesError::Parse { path, source },
    )
}

#[derive(Deserialize)]
struct RawDamageType {
    message_id: String,
    scaling: String,
    exhaustion: f32,
    effects: Option<String>,
    death_message_type: Option<String>,
}

impl RawDamageType {
    fn into_facts(self, id: Identifier) -> DamageTypeFacts {
        DamageTypeFacts {
            category: category_for_id(id.path()),
            id,
            message_id: self.message_id,
            scaling: self.scaling,
            exhaustion: self.exhaustion,
            effects: self.effects,
            death_message_type: self.death_message_type,
        }
    }
}

fn category_for_id(path: &str) -> DamageCategory {
    match path {
        "generic" => DamageCategory::Generic,
        "player_attack" => DamageCategory::PlayerAttack,
        "mob_attack" => DamageCategory::MobAttack,
        "arrow" | "trident" | "fireworks" | "mob_projectile" => DamageCategory::Projectile,
        "fall" => DamageCategory::Fall,
        "in_fire" | "on_fire" | "lava" | "hot_floor" => DamageCategory::Fire,
        "drown" => DamageCategory::Drowning,
        "in_wall" | "cramming" => DamageCategory::Suffocation,
        "starve" => DamageCategory::Starvation,
        "explosion" | "player_explosion" | "bad_respawn_point" => DamageCategory::Explosion,
        _ => DamageCategory::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn loads_damage_type_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("player_attack.json"),
            r#"{
              "exhaustion": 0.1,
              "message_id": "player",
              "scaling": "when_caused_by_living_non_player"
            }"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("lava.json"),
            r#"{
              "effects": "burning",
              "exhaustion": 0.1,
              "message_id": "lava",
              "scaling": "when_caused_by_living_non_player"
            }"#,
        )
        .unwrap();

        let table = load_damage_type_facts(tmp.path()).unwrap();

        let player_attack = table
            .get(&Identifier::parse("minecraft:player_attack").unwrap())
            .unwrap();
        assert_eq!(player_attack.category, DamageCategory::PlayerAttack);
        assert_eq!(player_attack.message_id, "player");
        assert_eq!(player_attack.exhaustion, 0.1);

        let lava = table
            .get(&Identifier::parse("minecraft:lava").unwrap())
            .unwrap();
        assert_eq!(lava.category, DamageCategory::Fire);
        assert_eq!(lava.effects.as_deref(), Some("burning"));
    }

    #[test]
    fn fallback_table_covers_m33_damage_sources() {
        let table = DamageTypeTable::with_solaris_fallbacks();

        for id in [
            "minecraft:generic",
            "minecraft:player_attack",
            "minecraft:mob_attack",
            "minecraft:arrow",
            "minecraft:fall",
            "minecraft:lava",
            "minecraft:drown",
            "minecraft:in_wall",
            "minecraft:starve",
            "minecraft:explosion",
        ] {
            assert!(table.get(&Identifier::parse(id).unwrap()).is_some(), "{id}");
        }
    }

    #[test]
    #[ignore = "requires local 26.1.2 damage type sidecars"]
    fn loads_real_damage_types_when_present() {
        let path = workspace_path("data/vanilla/data/minecraft/damage_type");
        assert!(
            path.is_dir(),
            "{} not present; run tools/extract-vanilla-data.sh",
            path.display()
        );

        let table = load_damage_type_facts(path).unwrap();

        let fall = table
            .get(&Identifier::parse("minecraft:fall").unwrap())
            .unwrap();
        assert_eq!(fall.category, DamageCategory::Fall);
        assert_eq!(fall.message_id, "fall");
        assert_eq!(fall.death_message_type.as_deref(), Some("fall_variants"));

        let player_attack = table
            .get(&Identifier::parse("minecraft:player_attack").unwrap())
            .unwrap();
        assert_eq!(player_attack.category, DamageCategory::PlayerAttack);
        assert_eq!(player_attack.exhaustion, 0.1);
    }

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }
}
