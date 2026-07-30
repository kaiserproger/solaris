use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::food::FoodEntry;
use crate::{Identifier, read_json_file, visit_json_files};

const REQUIRED_ITEM_COMPONENTS: &str = include_str!("../data/required_item_components.json");
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
    pub fn from_entries(entries: impl IntoIterator<Item = (Identifier, ItemFacts)>) -> Self {
        Self {
            items: entries.into_iter().collect(),
        }
    }

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
    pub weapon: bool,
    pub weapon_damage_per_attack: Option<u32>,
    pub weapon_disable_blocking_seconds: Option<f32>,
    pub blocks_attacks_disable_cooldown_scale: Option<f32>,
    pub attack_damage_modifier: Option<f32>,
    pub attack_speed_modifier: Option<f32>,
    pub attack_range: Option<AttackRangeFacts>,
    pub armor: Option<ItemArmorFacts>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackRangeFacts {
    pub min_reach: f32,
    pub max_reach: f32,
    pub min_creative_reach: f32,
    pub max_creative_reach: f32,
    pub hitbox_margin: f32,
    pub mob_factor: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseAction {
    Eat,
    Drink,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolFacts {
    pub default_mining_speed: Option<f32>,
    pub damage_per_block: Option<u32>,
    pub rules: Vec<ToolRuleFacts>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRuleFacts {
    pub blocks: Vec<String>,
    pub speed: Option<f32>,
    pub correct_for_drops: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemArmorFacts {
    pub slot: String,
    pub armor: f32,
    pub toughness: f32,
}

pub fn load_item_facts(
    report_dir: impl AsRef<Path>,
) -> Result<ItemFactsTable, ItemComponentsError> {
    let report_dir = report_dir.as_ref();
    if !report_dir.is_dir() {
        return Ok(ItemFactsTable::default());
    }

    let mut paths = Vec::new();
    visit_json_files(
        report_dir,
        &mut |path| {
            paths.push(path);
            Ok(())
        },
        &|path, source| ItemComponentsError::Io { path, source },
    )?;
    paths.sort();

    let mut items = BTreeMap::new();
    for path in paths {
        let id = id_from_path(report_dir, &path)?;
        let facts = load_one(&path)?;
        items.insert(id, facts);
    }
    Ok(ItemFactsTable { items })
}

#[must_use]
pub fn solaris_required_item_facts() -> ItemFactsTable {
    let raw: BTreeMap<String, RawEmbeddedItemFacts> =
        serde_json::from_str(REQUIRED_ITEM_COMPONENTS)
            .expect("embedded required item component JSON is valid");
    ItemFactsTable::from_entries(raw.into_iter().map(|(id, facts)| {
        (
            Identifier::parse(id).expect("embedded required item component id is valid"),
            facts.into_facts(),
        )
    }))
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
    let raw: RawItemComponents = read_json_file(
        path,
        &|path, source| ItemComponentsError::Io { path, source },
        &|path, source| ItemComponentsError::Parse { path, source },
    )?;
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
    #[serde(rename = "minecraft:weapon")]
    weapon: Option<RawWeapon>,
    #[serde(rename = "minecraft:blocks_attacks")]
    blocks_attacks: Option<RawBlocksAttacks>,
    #[serde(rename = "minecraft:attack_range")]
    attack_range: Option<RawAttackRange>,
    #[serde(default, rename = "minecraft:attribute_modifiers")]
    attribute_modifiers: Vec<RawAttributeModifier>,
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
            weapon,
            blocks_attacks,
            attack_range,
            attribute_modifiers,
        } = self.components;
        let attack_damage_modifier =
            find_mainhand_add_modifier(&attribute_modifiers, "minecraft:attack_damage");
        let attack_speed_modifier =
            find_mainhand_add_modifier(&attribute_modifiers, "minecraft:attack_speed");
        let armor = equippable.as_ref().and_then(|raw| {
            let armor = find_slot_add_modifier(&attribute_modifiers, "minecraft:armor", &raw.slot)?;
            let toughness = find_slot_add_modifier(
                &attribute_modifiers,
                "minecraft:armor_toughness",
                &raw.slot,
            )
            .unwrap_or(0.0);
            Some(ItemArmorFacts {
                slot: raw.slot.clone(),
                armor,
                toughness,
            })
        });
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
            tool: tool.map(RawTool::into_facts),
            equippable_slot: equippable.map(|raw| raw.slot),
            weapon: weapon.is_some(),
            weapon_damage_per_attack: weapon
                .as_ref()
                .map(|raw| raw.item_damage_per_attack.unwrap_or(1)),
            weapon_disable_blocking_seconds: weapon
                .as_ref()
                .and_then(|raw| raw.disable_blocking_for_seconds),
            blocks_attacks_disable_cooldown_scale: blocks_attacks
                .as_ref()
                .map(|raw| raw.disable_cooldown_scale.unwrap_or(1.0)),
            attack_damage_modifier,
            attack_speed_modifier,
            attack_range: attack_range.map(RawAttackRange::into_facts),
            armor,
        }
    }
}

#[derive(Deserialize)]
struct RawFood {
    nutrition: i32,
    saturation: f32,
}

#[derive(Default, Deserialize)]
struct RawEmbeddedItemFacts {
    max_stack_size: Option<u32>,
    max_damage: Option<u32>,
    food: Option<FoodEntry>,
    use_duration_ticks: Option<u32>,
    use_action: Option<RawEmbeddedUseAction>,
    weapon: Option<bool>,
    weapon_damage_per_attack: Option<u32>,
    weapon_disable_blocking_seconds: Option<f32>,
    blocks_attacks_disable_cooldown_scale: Option<f32>,
    attack_damage_modifier: Option<f32>,
    attack_speed_modifier: Option<f32>,
    attack_range: Option<RawAttackRange>,
    tool: Option<RawTool>,
}

impl RawEmbeddedItemFacts {
    fn into_facts(self) -> ItemFacts {
        ItemFacts {
            max_stack_size: self.max_stack_size,
            max_damage: self.max_damage,
            food: self.food,
            use_duration_ticks: self.use_duration_ticks,
            use_action: self.use_action.map(RawEmbeddedUseAction::into_use_action),
            tool: self.tool.map(RawTool::into_facts),
            equippable_slot: None,
            weapon: self.weapon.unwrap_or(false),
            weapon_damage_per_attack: self.weapon_damage_per_attack,
            weapon_disable_blocking_seconds: self.weapon_disable_blocking_seconds,
            blocks_attacks_disable_cooldown_scale: self.blocks_attacks_disable_cooldown_scale,
            attack_damage_modifier: self.attack_damage_modifier,
            attack_speed_modifier: self.attack_speed_modifier,
            attack_range: self.attack_range.map(RawAttackRange::into_facts),
            armor: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawEmbeddedUseAction {
    Eat,
    Drink,
    Other,
}

impl RawEmbeddedUseAction {
    const fn into_use_action(self) -> UseAction {
        match self {
            Self::Eat => UseAction::Eat,
            Self::Drink => UseAction::Drink,
            Self::Other => UseAction::Other,
        }
    }
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
    #[serde(default)]
    rules: Vec<RawToolRule>,
}

impl RawTool {
    fn into_facts(self) -> ToolFacts {
        ToolFacts {
            default_mining_speed: self.default_mining_speed,
            damage_per_block: self.damage_per_block,
            rules: self
                .rules
                .into_iter()
                .map(|rule| ToolRuleFacts {
                    blocks: rule.blocks.into_vec(),
                    speed: rule.speed,
                    correct_for_drops: rule.correct_for_drops,
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct RawToolRule {
    blocks: RawToolBlocks,
    speed: Option<f32>,
    correct_for_drops: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawToolBlocks {
    One(String),
    Many(Vec<String>),
}

impl RawToolBlocks {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(block) => vec![block],
            Self::Many(blocks) => blocks,
        }
    }
}

#[derive(Deserialize)]
struct RawEquippable {
    slot: String,
}

#[derive(Deserialize)]
struct RawWeapon {
    item_damage_per_attack: Option<u32>,
    disable_blocking_for_seconds: Option<f32>,
}

#[derive(Deserialize)]
struct RawBlocksAttacks {
    disable_cooldown_scale: Option<f32>,
}

#[derive(Deserialize)]
struct RawAttackRange {
    #[serde(default)]
    min_reach: f32,
    #[serde(default = "default_attack_max_reach")]
    max_reach: f32,
    #[serde(default)]
    min_creative_reach: f32,
    #[serde(default = "default_attack_creative_max_reach")]
    max_creative_reach: f32,
    #[serde(default = "default_attack_hitbox_margin")]
    hitbox_margin: f32,
    #[serde(default = "default_attack_mob_factor")]
    mob_factor: f32,
}

impl RawAttackRange {
    fn into_facts(self) -> AttackRangeFacts {
        AttackRangeFacts {
            min_reach: self.min_reach,
            max_reach: self.max_reach,
            min_creative_reach: self.min_creative_reach,
            max_creative_reach: self.max_creative_reach,
            hitbox_margin: self.hitbox_margin,
            mob_factor: self.mob_factor,
        }
    }
}

const fn default_attack_max_reach() -> f32 {
    3.0
}

const fn default_attack_creative_max_reach() -> f32 {
    5.0
}

const fn default_attack_hitbox_margin() -> f32 {
    0.3
}

const fn default_attack_mob_factor() -> f32 {
    1.0
}

#[derive(Deserialize)]
struct RawAttributeModifier {
    #[serde(rename = "type")]
    kind: String,
    amount: f32,
    operation: String,
    slot: Option<String>,
}

fn find_mainhand_add_modifier(modifiers: &[RawAttributeModifier], kind: &str) -> Option<f32> {
    find_slot_add_modifier(modifiers, kind, "mainhand")
}

fn find_slot_add_modifier(
    modifiers: &[RawAttributeModifier],
    kind: &str,
    slot: &str,
) -> Option<f32> {
    modifiers
        .iter()
        .find(|modifier| {
            normalize_id(&modifier.kind) == normalize_id(kind)
                && normalize_id(&modifier.operation) == "add_value"
                && modifier
                    .slot
                    .as_deref()
                    .is_some_and(|value| normalize_id(value) == normalize_id(slot))
        })
        .map(|modifier| modifier.amount)
}

fn normalize_id(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
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
        fs::write(
            tmp.path().join("diamond_sword.json"),
            r#"{
              "components": {
                "minecraft:max_damage": 1561,
                "minecraft:max_stack_size": 1,
                "minecraft:attribute_modifiers": [
                  {
                    "type": "minecraft:attack_damage",
                    "amount": 6.0,
                    "id": "minecraft:base_attack_damage",
                    "operation": "add_value",
                    "slot": "mainhand"
                  },
                  {
                    "type": "minecraft:attack_speed",
                    "amount": -2.4,
                    "id": "minecraft:base_attack_speed",
                    "operation": "add_value",
                    "slot": "mainhand"
                  }
                ],
                "minecraft:weapon": {}
              }
            }"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("iron_chestplate.json"),
            r#"{
              "components": {
                "minecraft:max_damage": 240,
                "minecraft:max_stack_size": 1,
                "minecraft:equippable": { "slot": "chest" },
                "minecraft:attribute_modifiers": [
                  {
                    "type": "minecraft:armor",
                    "amount": 6.0,
                    "operation": "add_value",
                    "slot": "chest"
                  },
                  {
                    "type": "minecraft:armor_toughness",
                    "amount": 0.0,
                    "operation": "add_value",
                    "slot": "chest"
                  }
                ]
              }
            }"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("wooden_pickaxe.json"),
            r##"{
              "components": {
                "minecraft:max_damage": 59,
                "minecraft:tool": {
                  "rules": [
                    {
                      "blocks": "#minecraft:incorrect_for_wooden_tool",
                      "correct_for_drops": false
                    },
                    {
                      "blocks": ["minecraft:stone", "minecraft:cobblestone"],
                      "speed": 2.0,
                      "correct_for_drops": true
                    }
                  ]
                }
              }
            }"##,
        )
        .unwrap();
        fs::write(
            tmp.path().join("wooden_spear.json"),
            r#"{
              "components": {
                "minecraft:attack_range": {
                  "min_reach": 2.0,
                  "max_reach": 4.5,
                  "min_creative_reach": 2.0,
                  "max_creative_reach": 6.5,
                  "hitbox_margin": 0.125,
                  "mob_factor": 0.5
                }
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

        let sword = facts
            .get(&Identifier::parse("minecraft:diamond_sword").unwrap())
            .unwrap();
        assert_eq!(sword.max_damage, Some(1561));
        assert!(sword.weapon);
        assert_eq!(sword.weapon_damage_per_attack, Some(1));
        assert_eq!(sword.attack_damage_modifier, Some(6.0));
        assert_eq!(sword.attack_speed_modifier, Some(-2.4));

        let chestplate = facts
            .get(&Identifier::parse("minecraft:iron_chestplate").unwrap())
            .unwrap();
        assert_eq!(chestplate.max_damage, Some(240));
        assert_eq!(chestplate.equippable_slot.as_deref(), Some("chest"));
        assert_eq!(
            chestplate.armor,
            Some(ItemArmorFacts {
                slot: "chest".to_string(),
                armor: 6.0,
                toughness: 0.0,
            })
        );

        let pickaxe = facts
            .get(&Identifier::parse("minecraft:wooden_pickaxe").unwrap())
            .and_then(|facts| facts.tool.as_ref())
            .expect("wooden pickaxe tool facts");
        assert_eq!(pickaxe.default_mining_speed, None);
        assert_eq!(pickaxe.rules.len(), 2);
        assert_eq!(
            pickaxe.rules[0],
            ToolRuleFacts {
                blocks: vec!["#minecraft:incorrect_for_wooden_tool".to_string()],
                speed: None,
                correct_for_drops: Some(false),
            }
        );
        assert_eq!(
            pickaxe.rules[1],
            ToolRuleFacts {
                blocks: vec![
                    "minecraft:stone".to_string(),
                    "minecraft:cobblestone".to_string(),
                ],
                speed: Some(2.0),
                correct_for_drops: Some(true),
            }
        );

        let spear = facts
            .get(&Identifier::parse("minecraft:wooden_spear").unwrap())
            .and_then(|facts| facts.attack_range)
            .expect("wooden spear attack range");
        assert_eq!(
            spear,
            AttackRangeFacts {
                min_reach: 2.0,
                max_reach: 4.5,
                min_creative_reach: 2.0,
                max_creative_reach: 6.5,
                hitbox_margin: 0.125,
                mob_factor: 0.5,
            }
        );
    }

    #[test]
    fn missing_component_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let facts = load_item_facts(tmp.path().join("missing")).unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn embedded_required_item_facts_cover_basic_wood_and_stone_tools() {
        let facts = solaris_required_item_facts();
        for (id, max_damage) in [
            ("minecraft:wooden_pickaxe", 59),
            ("minecraft:wooden_axe", 59),
            ("minecraft:wooden_shovel", 59),
            ("minecraft:wooden_sword", 59),
            ("minecraft:wooden_hoe", 59),
            ("minecraft:stone_pickaxe", 131),
            ("minecraft:stone_axe", 131),
            ("minecraft:stone_shovel", 131),
            ("minecraft:stone_sword", 131),
            ("minecraft:stone_hoe", 131),
        ] {
            let item = Identifier::parse(id).unwrap();
            let fact = facts.get(&item).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(fact.max_damage, Some(max_damage), "{id}");
            assert!(fact.attack_damage_modifier.is_some(), "{id}");
        }
    }

    #[test]
    fn embedded_required_item_facts_cover_shield_disable_contract() {
        let facts = solaris_required_item_facts();
        for id in ["minecraft:wooden_axe", "minecraft:stone_axe"] {
            let fact = facts
                .get(&Identifier::parse(id).unwrap())
                .unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(fact.weapon_disable_blocking_seconds, Some(5.0), "{id}");
        }
        let shield = facts
            .get(&Identifier::parse("minecraft:shield").unwrap())
            .expect("embedded shield facts");
        assert_eq!(shield.max_damage, Some(336));
        assert_eq!(shield.max_stack_size, Some(1));
        assert_eq!(shield.blocks_attacks_disable_cooldown_scale, Some(1.0));
    }

    #[test]
    fn embedded_required_item_facts_cover_playable_cooked_food() {
        let facts = solaris_required_item_facts();
        for (id, food, saturation) in [
            ("minecraft:cooked_beef", 8, 12.8),
            ("minecraft:cooked_porkchop", 8, 12.8),
            ("minecraft:cooked_chicken", 6, 7.2),
        ] {
            let item = Identifier::parse(id).unwrap();
            let fact = facts.get(&item).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(fact.food, Some(FoodEntry { food, saturation }), "{id}");
            assert_eq!(fact.use_duration_ticks, Some(32), "{id}");
            assert_eq!(fact.use_action, Some(UseAction::Eat), "{id}");
        }
    }

    #[test]
    fn embedded_required_item_facts_cover_shears_durability() {
        let facts = solaris_required_item_facts();
        let shears = facts
            .get(&Identifier::parse("minecraft:shears").unwrap())
            .expect("embedded shears facts");
        assert_eq!(shears.max_damage, Some(238));
        assert_eq!(shears.max_stack_size, Some(1));
    }

    #[test]
    fn embedded_required_item_facts_cover_spear_attack_range() {
        let facts = solaris_required_item_facts();
        for id in [
            "minecraft:wooden_spear",
            "minecraft:stone_spear",
            "minecraft:copper_spear",
            "minecraft:iron_spear",
            "minecraft:golden_spear",
            "minecraft:diamond_spear",
            "minecraft:netherite_spear",
        ] {
            let range = facts
                .get(&Identifier::parse(id).unwrap())
                .and_then(|facts| facts.attack_range)
                .unwrap_or_else(|| panic!("missing attack range for {id}"));
            assert_eq!(range.max_reach, 4.5, "{id}");
            assert_eq!(range.max_creative_reach, 6.5, "{id}");
            assert_eq!(range.hitbox_margin, 0.125, "{id}");
        }
    }

    #[test]
    #[ignore = "requires local 26.1.2 item component reports"]
    fn loads_real_apple_and_bread_components_when_present() {
        let path = workspace_path("data/vanilla/reports/minecraft/components/item");
        assert!(
            path.is_dir(),
            "{} not present; run tools/extract-vanilla-data.sh",
            path.display()
        );

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

    #[test]
    #[ignore = "requires local 26.1.2 item component reports"]
    fn loads_real_combat_components_when_present() {
        let path = workspace_path("data/vanilla/reports/minecraft/components/item");
        assert!(
            path.is_dir(),
            "{} not present; run tools/extract-vanilla-data.sh",
            path.display()
        );

        let facts = load_item_facts(path).unwrap();

        let sword = facts
            .get(&Identifier::parse("minecraft:diamond_sword").unwrap())
            .unwrap();
        assert_eq!(sword.max_damage, Some(1561));
        assert!(sword.weapon);
        assert_eq!(sword.weapon_damage_per_attack, Some(1));
        assert_eq!(sword.attack_damage_modifier, Some(6.0));
        assert!(sword.attack_speed_modifier.is_some());

        let axe = facts
            .get(&Identifier::parse("minecraft:iron_axe").unwrap())
            .unwrap();
        assert!(axe.weapon);
        assert_eq!(axe.weapon_damage_per_attack, Some(2));
        assert_eq!(axe.weapon_disable_blocking_seconds, Some(5.0));
        assert_eq!(axe.attack_damage_modifier, Some(8.0));

        let shield = facts
            .get(&Identifier::parse("minecraft:shield").unwrap())
            .unwrap();
        assert_eq!(shield.blocks_attacks_disable_cooldown_scale, Some(1.0));

        let bow = facts
            .get(&Identifier::parse("minecraft:bow").unwrap())
            .unwrap();
        assert_eq!(bow.max_damage, Some(384));
        assert_eq!(bow.max_stack_size, Some(1));

        let chestplate = facts
            .get(&Identifier::parse("minecraft:iron_chestplate").unwrap())
            .unwrap();
        assert_eq!(chestplate.max_damage, Some(240));
        assert_eq!(chestplate.equippable_slot.as_deref(), Some("chest"));
        assert_eq!(
            chestplate.armor,
            Some(ItemArmorFacts {
                slot: "chest".to_string(),
                armor: 6.0,
                toughness: 0.0,
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

    #[test]
    #[ignore = "requires local 26.1.2 item component reports"]
    fn loads_real_ordered_tool_rules_when_present() {
        let path = workspace_path("data/vanilla/reports/minecraft/components/item");
        assert!(
            path.is_dir(),
            "{} not present; run tools/extract-vanilla-data.sh",
            path.display()
        );

        let facts = load_item_facts(path).unwrap();
        let pickaxe = facts
            .get(&Identifier::parse("minecraft:wooden_pickaxe").unwrap())
            .and_then(|facts| facts.tool.as_ref())
            .expect("wooden pickaxe tool facts");
        assert_eq!(pickaxe.default_mining_speed, None);
        assert_eq!(pickaxe.damage_per_block, None);
        assert_eq!(pickaxe.rules.len(), 2);
        assert_eq!(
            pickaxe.rules[0].blocks,
            ["#minecraft:incorrect_for_wooden_tool"]
        );
        assert_eq!(pickaxe.rules[0].correct_for_drops, Some(false));
        assert_eq!(pickaxe.rules[1].blocks, ["#minecraft:mineable/pickaxe"]);
        assert_eq!(pickaxe.rules[1].speed, Some(2.0));
        assert_eq!(pickaxe.rules[1].correct_for_drops, Some(true));
    }
}
