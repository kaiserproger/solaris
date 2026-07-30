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
