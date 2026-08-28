//! Per-block-state mining metadata extracted from the target vanilla server.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::read_json_file;

#[derive(Debug, Error)]
pub enum BlockMiningError {
    #[error("block_mining.json not found at {0}; did you run tools/extract-block-mining.sh?")]
    Missing(PathBuf),
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "entries length {entries} does not match max_state_id {max_state_id} (expected {expected})"
    )]
    LengthMismatch {
        entries: usize,
        max_state_id: u32,
        expected: usize,
    },
    #[error(
        "state {state_id} has invalid destroy speed {value}; expected -1 or a finite non-negative value"
    )]
    InvalidDestroySpeed { state_id: u32, value: f32 },
    #[error("state {state_id} has requires_correct_tool = {value}; expected 0 or 1")]
    InvalidBool { state_id: u32, value: u8 },
}

#[derive(Debug, Clone, Default)]
pub struct BlockMiningTable {
    pub version: String,
    destroy_speed: Vec<f32>,
    requires_correct_tool: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockMiningFacts {
    pub destroy_speed: f32,
    pub requires_correct_tool_for_drops: bool,
}

pub const VANILLA_SUBMERGED_MINING_SPEED: f32 = 0.2;
const FALLBACK_UNKNOWN_DESTROY_SPEED: f32 = 0.8;

#[must_use]
pub fn block_break_is_denied(block_path: &str) -> bool {
    matches!(block_path, "bedrock" | "barrier" | "end_portal_frame")
}

#[must_use]
pub fn fallback_mining_facts(block_path: &str) -> BlockMiningFacts {
    let (destroy_speed, requires_correct_tool_for_drops) = match block_path {
        "bedrock" | "barrier" | "end_portal_frame" => (-1.0, false),
        "air"
        | "cave_air"
        | "void_air"
        | "short_grass"
        | "tall_grass"
        | "wheat"
        | "carrots"
        | "potatoes"
        | "beetroots"
        | "nether_wart"
        | "pumpkin_stem"
        | "melon_stem"
        | "attached_pumpkin_stem"
        | "attached_melon_stem"
        | "sweet_berry_bush"
        | "sugar_cane"
        | "kelp"
        | "kelp_plant"
        | "torch"
        | "wall_torch" => (0.0, false),
        "cocoa" => (0.2, false),
        "stone" | "granite" | "diorite" | "andesite" | "calcite" | "tuff" => (1.5, true),
        "cobblestone" | "mossy_cobblestone" => (2.0, true),
        "deepslate" => (3.0, true),
        "cobbled_deepslate" => (3.5, true),
        "obsidian" | "crying_obsidian" => (50.0, true),
        "ancient_debris" => (30.0, true),
        "dirt" | "coarse_dirt" | "rooted_dirt" | "podzol" | "sand" | "red_sand" => (0.5, false),
        "grass_block" | "gravel" | "clay" => (0.6, false),
        "crafting_table" | "chest" | "trapped_chest" => (2.5, false),
        "furnace" | "blast_furnace" | "smoker" => (3.5, true),
        path if path.starts_with("deepslate_") && path.ends_with("_ore") => (4.5, true),
        path if path.ends_with("_ore") => (3.0, true),
        path if path.ends_with("_log")
            || path.ends_with("_wood")
            || path.ends_with("_stem")
            || path.ends_with("_hyphae")
            || path.ends_with("_planks") =>
        {
            (2.0, false)
        }
        _ => (FALLBACK_UNKNOWN_DESTROY_SPEED, false),
    };
    BlockMiningFacts {
        destroy_speed,
        requires_correct_tool_for_drops,
    }
}

#[must_use]
pub fn fallback_tool_suffix_for_path(block_path: &str) -> Option<&'static str> {
    let facts = fallback_mining_facts(block_path);
    if facts.requires_correct_tool_for_drops
        || matches!(block_path, "furnace" | "blast_furnace" | "smoker")
    {
        return Some("_pickaxe");
    }
    if block_path.ends_with("_log")
        || block_path.ends_with("_wood")
        || block_path.ends_with("_stem")
        || block_path.ends_with("_hyphae")
        || block_path.ends_with("_planks")
        || matches!(block_path, "crafting_table" | "chest" | "trapped_chest")
    {
        return Some("_axe");
    }
    if matches!(
        block_path,
        "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "podzol"
            | "grass_block"
            | "sand"
            | "red_sand"
            | "gravel"
            | "clay"
    ) {
        return Some("_shovel");
    }
    None
}

#[must_use]
pub fn fallback_tool_mining_speed(tool_path: Option<&str>, required_suffix: Option<&str>) -> f32 {
    let Some(tool_path) = tool_path else {
        return 1.0;
    };
    if required_suffix.is_some_and(|suffix| !tool_path.ends_with(suffix)) {
        return 1.0;
    }
    for (material, speed) in [
        ("wooden_", 2.0),
        ("stone_", 4.0),
        ("copper_", 5.0),
        ("iron_", 6.0),
        ("diamond_", 8.0),
        ("netherite_", 9.0),
        ("golden_", 12.0),
    ] {
        if tool_path.starts_with(material) {
            return speed;
        }
    }
    1.0
}

fn pickaxe_tier(tool_path: &str) -> Option<u8> {
    let material = tool_path.strip_suffix("_pickaxe")?;
    match material {
        "wooden" | "golden" => Some(0),
        "stone" | "copper" => Some(1),
        "iron" => Some(2),
        "diamond" => Some(3),
        "netherite" => Some(4),
        _ => None,
    }
}

fn required_pickaxe_tier_for_drop(block_path: &str) -> Option<u8> {
    let block_path = block_path.strip_prefix("deepslate_").unwrap_or(block_path);
    match block_path {
        "stone" | "cobblestone" | "deepslate" | "cobbled_deepslate" | "coal_ore"
        | "nether_gold_ore" | "nether_quartz_ore" => Some(0),
        "iron_ore" | "copper_ore" | "lapis_ore" => Some(1),
        "diamond_ore" | "emerald_ore" | "gold_ore" | "redstone_ore" => Some(2),
        "obsidian" | "crying_obsidian" | "ancient_debris" => Some(3),
        _ => None,
    }
}

#[must_use]
pub fn fallback_tool_allows_block_drop(block_path: &str, tool_path: Option<&str>) -> bool {
    let Some(required_tier) = required_pickaxe_tier_for_drop(block_path) else {
        return true;
    };
    tool_path
        .and_then(pickaxe_tier)
        .is_some_and(|tier| tier >= required_tier)
}

#[must_use]
pub fn destroy_progress_per_tick(
    destroy_speed: f32,
    mut item_speed: f32,
    has_correct_tool_for_drops: bool,
    on_ground: bool,
    eye_in_water: bool,
) -> f32 {
    if destroy_speed < 0.0 {
        return 0.0;
    }
    if destroy_speed == 0.0 {
        return f32::INFINITY;
    }
    if !item_speed.is_finite() || item_speed < 0.0 {
        item_speed = 1.0;
    }
    if eye_in_water {
        item_speed *= VANILLA_SUBMERGED_MINING_SPEED;
    }
    if !on_ground {
        item_speed /= 5.0;
    }
    let divisor = if has_correct_tool_for_drops {
        30.0
    } else {
        100.0
    };
    item_speed / destroy_speed / divisor
}

impl BlockMiningTable {
    #[must_use]
    pub fn from_arrays(
        version: impl Into<String>,
        destroy_speed: Vec<f32>,
        requires_correct_tool: Vec<bool>,
    ) -> Self {
        assert_eq!(destroy_speed.len(), requires_correct_tool.len());
        Self {
            version: version.into(),
            destroy_speed,
            requires_correct_tool,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.destroy_speed.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.destroy_speed.is_empty()
    }

    #[must_use]
    pub fn destroy_speed(&self, state_id: u32) -> Option<f32> {
        self.destroy_speed.get(state_id as usize).copied()
    }

    #[must_use]
    pub fn requires_correct_tool_for_drops(&self, state_id: u32) -> Option<bool> {
        self.requires_correct_tool.get(state_id as usize).copied()
    }

    #[must_use]
    pub fn facts(&self, state_id: u32) -> Option<BlockMiningFacts> {
        Some(BlockMiningFacts {
            destroy_speed: self.destroy_speed(state_id)?,
            requires_correct_tool_for_drops: self.requires_correct_tool_for_drops(state_id)?,
        })
    }
}

#[derive(Deserialize)]
struct RawBlockMiningTable {
    version: String,
    max_state_id: u32,
    entries: Vec<(f32, u8)>,
}

pub fn load(path: impl AsRef<Path>) -> Result<BlockMiningTable, BlockMiningError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(BlockMiningError::Missing(path.to_path_buf()));
    }
    let raw: RawBlockMiningTable = read_json_file(
        path,
        &|path, source| BlockMiningError::Io { path, source },
        &|path, source| BlockMiningError::Parse { path, source },
    )?;
    let expected = usize::try_from(u64::from(raw.max_state_id) + 1).unwrap_or(usize::MAX);
    if raw.entries.len() != expected {
        return Err(BlockMiningError::LengthMismatch {
            entries: raw.entries.len(),
            max_state_id: raw.max_state_id,
            expected,
        });
    }

    let mut destroy_speed = Vec::with_capacity(expected);
    let mut requires_correct_tool = Vec::with_capacity(expected);
    for (index, (speed, requires_correct)) in raw.entries.into_iter().enumerate() {
        if !speed.is_finite() || speed < -1.0 {
            return Err(BlockMiningError::InvalidDestroySpeed {
                state_id: index as u32,
                value: speed,
            });
        }
        if requires_correct > 1 {
            return Err(BlockMiningError::InvalidBool {
                state_id: index as u32,
                value: requires_correct,
            });
        }
        destroy_speed.push(speed);
        requires_correct_tool.push(requires_correct == 1);
    }

    Ok(BlockMiningTable {
        version: raw.version,
        destroy_speed,
        requires_correct_tool,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_json(dir: &TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("block_mining.json");
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn fallback_rules_cover_common_tool_families_and_drop_tiers() {
        assert!(block_break_is_denied("bedrock"));
        assert!(block_break_is_denied("barrier"));
        assert!(!block_break_is_denied("stone"));
        assert_eq!(fallback_mining_facts("stone").destroy_speed, 1.5);
        assert_eq!(fallback_tool_suffix_for_path("oak_log"), Some("_axe"));
        assert_eq!(fallback_tool_suffix_for_path("dirt"), Some("_shovel"));
        assert_eq!(
            fallback_tool_mining_speed(Some("iron_pickaxe"), Some("_pickaxe")),
            6.0
        );
        assert_eq!(
            fallback_tool_mining_speed(Some("iron_shovel"), Some("_pickaxe")),
            1.0
        );
        for (tool, speed) in [
            ("wooden_pickaxe", 2.0),
            ("stone_pickaxe", 4.0),
            ("copper_pickaxe", 5.0),
            ("iron_pickaxe", 6.0),
            ("diamond_pickaxe", 8.0),
            ("netherite_pickaxe", 9.0),
            ("golden_pickaxe", 12.0),
        ] {
            assert_eq!(
                fallback_tool_mining_speed(Some(tool), Some("_pickaxe")),
                speed,
                "wrong fallback mining speed for {tool}"
            );
        }
        assert_eq!(fallback_mining_facts("podzol").destroy_speed, 0.5);
        assert_eq!(
            fallback_mining_facts("unknown_custom_block").destroy_speed,
            FALLBACK_UNKNOWN_DESTROY_SPEED
        );
        assert_eq!(fallback_mining_facts("nether_wart").destroy_speed, 0.0);
        assert!(!fallback_tool_allows_block_drop(
            "iron_ore",
            Some("wooden_pickaxe")
        ));
        assert!(fallback_tool_allows_block_drop(
            "deepslate_iron_ore",
            Some("stone_pickaxe")
        ));
        assert!(!fallback_tool_allows_block_drop(
            "obsidian",
            Some("iron_pickaxe")
        ));
        assert!(fallback_tool_allows_block_drop(
            "obsidian",
            Some("diamond_pickaxe")
        ));
    }

    #[test]
    fn destroy_progress_applies_submerged_airborne_and_instant_rules() {
        let base = destroy_progress_per_tick(1.5, 6.0, true, true, false);
        assert_eq!(base, 6.0 / 1.5 / 30.0);
        assert!(
            (destroy_progress_per_tick(1.5, 6.0, true, true, true)
                - base * VANILLA_SUBMERGED_MINING_SPEED)
                .abs()
                < 1.0e-6
        );
        assert!(
            (destroy_progress_per_tick(1.5, 6.0, true, false, false) - base / 5.0).abs() < 1.0e-6
        );
        assert_eq!(destroy_progress_per_tick(-1.0, 6.0, true, true, false), 0.0);
        assert!(destroy_progress_per_tick(0.0, 1.0, false, true, false).is_infinite());
    }

    #[test]
    fn loads_synthetic_table() {
        let dir = TempDir::new().unwrap();
        let table = load(write_json(
            &dir,
            r#"{
                "version": "26.1.2-test",
                "max_state_id": 2,
                "entries": [[0.0,0],[1.5,1],[-1.0,0]]
            }"#,
        ))
        .unwrap();

        assert_eq!(table.version, "26.1.2-test");
        assert_eq!(table.destroy_speed(0), Some(0.0));
        assert_eq!(table.destroy_speed(1), Some(1.5));
        assert_eq!(table.destroy_speed(2), Some(-1.0));
        assert_eq!(table.requires_correct_tool_for_drops(0), Some(false));
        assert_eq!(table.requires_correct_tool_for_drops(1), Some(true));
    }

    #[test]
    fn rejects_length_mismatch() {
        let dir = TempDir::new().unwrap();
        let error = load(write_json(
            &dir,
            r#"{"version":"v","max_state_id":1,"entries":[[1.0,0]]}"#,
        ))
        .unwrap_err();
        assert!(matches!(error, BlockMiningError::LengthMismatch { .. }));
    }

    #[test]
    fn rejects_invalid_boolean() {
        let dir = TempDir::new().unwrap();
        let error = load(write_json(
            &dir,
            r#"{"version":"v","max_state_id":0,"entries":[[1.0,2]]}"#,
        ))
        .unwrap_err();
        assert!(matches!(error, BlockMiningError::InvalidBool { .. }));
    }

    #[test]
    #[ignore = "requires local 26.1.2 block mining and blocks reports"]
    fn real_table_matches_vanilla_2612_blocks() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        let blocks_path = workspace.join("data/vanilla/reports/blocks.json");
        let mining_path = workspace.join("data/vanilla/reports/block_mining.json");
        assert!(
            blocks_path.is_file() && mining_path.is_file(),
            "need both {} and {}; run tools/extract-vanilla-data.sh",
            blocks_path.display(),
            mining_path.display()
        );

        let blocks = crate::blocks::load_blocks_report(blocks_path).unwrap();
        let table = load(mining_path).unwrap();
        assert_eq!(table.version, "26.1.2");
        assert_eq!(table.len(), 29_873);

        let state_id = |name: &str| {
            blocks
                .iter()
                .find(|block| block.id.as_str() == name)
                .and_then(|block| block.states.iter().find(|state| state.default))
                .unwrap_or_else(|| panic!("default state for {name} missing"))
                .id
        };
        for (name, speed, requires_correct_tool) in [
            ("minecraft:air", 0.0, false),
            ("minecraft:stone", 1.5, true),
            ("minecraft:oak_log", 2.0, false),
            ("minecraft:dirt", 0.5, false),
            ("minecraft:wheat", 0.0, false),
            ("minecraft:obsidian", 50.0, true),
            ("minecraft:bedrock", -1.0, false),
            ("minecraft:deepslate_iron_ore", 4.5, true),
        ] {
            let state = state_id(name);
            assert_eq!(table.destroy_speed(state), Some(speed), "{name}");
            assert_eq!(
                table.requires_correct_tool_for_drops(state),
                Some(requires_correct_tool),
                "{name}"
            );
        }
    }
}
