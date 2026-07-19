//! Per-block-state explosion resistance extracted from vanilla 26.1.2.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::read_json_file;

const EXPECTED_VERSION: &str = "26.1.2";
const EXPECTED_MAX_STATE_ID: u32 = 29_872;
const EXPECTED_STATE_COUNT: usize = 29_873;

#[derive(Debug, Error)]
pub enum BlockExplosionError {
    #[error("block_explosion.json not found at {0}; did you run tools/extract-block-explosion.sh?")]
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
    #[error("unsupported block explosion report version {version}; expected 26.1.2")]
    UnsupportedVersion { version: String },
    #[error("block explosion report max_state_id {actual} does not match 26.1.2 value {expected}")]
    UnexpectedMaxStateId { actual: u32, expected: u32 },
    #[error(
        "entries length {entries} does not match max_state_id {max_state_id} (expected {expected})"
    )]
    LengthMismatch {
        entries: usize,
        max_state_id: u32,
        expected: usize,
    },
    #[error("state {state_id} has invalid explosion resistance {value}")]
    InvalidResistance { state_id: u32, value: f64 },
}

#[derive(Debug, Clone)]
pub struct BlockExplosionTable {
    resistance: Vec<f32>,
}

impl BlockExplosionTable {
    pub fn from_resistances(resistance: Vec<f32>) -> Result<Self, BlockExplosionError> {
        if resistance.len() != EXPECTED_STATE_COUNT {
            return Err(BlockExplosionError::LengthMismatch {
                entries: resistance.len(),
                max_state_id: EXPECTED_MAX_STATE_ID,
                expected: EXPECTED_STATE_COUNT,
            });
        }
        for (state_id, value) in resistance.iter().copied().enumerate() {
            if value < 0.0 || !value.is_finite() {
                return Err(BlockExplosionError::InvalidResistance {
                    state_id: u32::try_from(state_id).expect("validated state ID fits u32"),
                    value: f64::from(value),
                });
            }
        }
        Ok(Self { resistance })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.resistance.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resistance.is_empty()
    }

    #[must_use]
    pub fn resistance(&self, state_id: u32) -> Option<f32> {
        self.resistance.get(state_id as usize).copied()
    }
}

#[derive(Deserialize)]
struct RawBlockExplosionTable {
    version: String,
    max_state_id: u32,
    entries: Vec<f64>,
}

pub fn load_block_explosion_report(
    path: impl AsRef<Path>,
) -> Result<BlockExplosionTable, BlockExplosionError> {
    let path = path.as_ref();
    match std::fs::metadata(path) {
        Ok(_) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(BlockExplosionError::Missing(path.to_path_buf()));
        }
        Err(source) => {
            return Err(BlockExplosionError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }

    let raw: RawBlockExplosionTable = read_json_file(
        path,
        &|path, source| BlockExplosionError::Io { path, source },
        &|path, source| BlockExplosionError::Parse { path, source },
    )?;
    if raw.version != EXPECTED_VERSION {
        return Err(BlockExplosionError::UnsupportedVersion {
            version: raw.version,
        });
    }
    if raw.max_state_id != EXPECTED_MAX_STATE_ID {
        return Err(BlockExplosionError::UnexpectedMaxStateId {
            actual: raw.max_state_id,
            expected: EXPECTED_MAX_STATE_ID,
        });
    }

    let expected = EXPECTED_STATE_COUNT;
    if raw.entries.len() != expected {
        return Err(BlockExplosionError::LengthMismatch {
            entries: raw.entries.len(),
            max_state_id: raw.max_state_id,
            expected,
        });
    }

    let mut resistance = Vec::with_capacity(expected);
    for (state_id, value) in raw.entries.into_iter().enumerate() {
        let resistance_value = value as f32;
        if value < 0.0 || !resistance_value.is_finite() {
            return Err(BlockExplosionError::InvalidResistance {
                state_id: u32::try_from(state_id).expect("validated state ID fits u32"),
                value,
            });
        }
        resistance.push(resistance_value);
    }

    BlockExplosionTable::from_resistances(resistance)
}

#[cfg(test)]
mod tests {
    use super::{
        BlockExplosionError, EXPECTED_MAX_STATE_ID, EXPECTED_STATE_COUNT,
        load_block_explosion_report,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn write_json(dir: &TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("block_explosion.json");
        fs::write(&path, body).unwrap();
        path
    }

    fn write_exact_json(dir: &TempDir, overrides: &[(usize, f64)]) -> PathBuf {
        let mut entries = vec![0.0; EXPECTED_STATE_COUNT];
        for &(state_id, value) in overrides {
            entries[state_id] = value;
        }
        write_json(
            dir,
            &serde_json::json!({
                "version": "26.1.2",
                "max_state_id": EXPECTED_MAX_STATE_ID,
                "entries": entries,
            })
            .to_string(),
        )
    }

    fn workspace_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(relative)
    }

    #[test]
    fn loads_exact_2612_table() {
        let dir = TempDir::new().unwrap();
        let table =
            load_block_explosion_report(write_exact_json(&dir, &[(1, 0.5), (2, 1200.0)])).unwrap();

        assert_eq!(table.len(), EXPECTED_STATE_COUNT);
        assert!(!table.is_empty());
        assert_eq!(table.resistance(0), Some(0.0));
        assert_eq!(table.resistance(1), Some(0.5));
        assert_eq!(table.resistance(2), Some(1200.0));
        assert_eq!(table.resistance(EXPECTED_MAX_STATE_ID), Some(0.0));
        assert_eq!(table.resistance(EXPECTED_MAX_STATE_ID + 1), None);
    }

    #[test]
    fn rejects_unsupported_version() {
        let dir = TempDir::new().unwrap();
        let error = load_block_explosion_report(write_json(
            &dir,
            r#"{"version":"26.1.3","max_state_id":0,"entries":[0.0]}"#,
        ))
        .unwrap_err();

        assert!(matches!(
            error,
            BlockExplosionError::UnsupportedVersion { ref version } if version == "26.1.3"
        ));
    }

    #[test]
    fn rejects_length_mismatch() {
        let dir = TempDir::new().unwrap();
        let entries = vec![0.0; EXPECTED_STATE_COUNT - 1];
        let error = load_block_explosion_report(write_json(
            &dir,
            &serde_json::json!({
                "version": "26.1.2",
                "max_state_id": EXPECTED_MAX_STATE_ID,
                "entries": entries,
            })
            .to_string(),
        ))
        .unwrap_err();

        assert!(matches!(error, BlockExplosionError::LengthMismatch { .. }));
    }

    #[test]
    fn rejects_truncated_2612_state_range() {
        let dir = TempDir::new().unwrap();
        let error = load_block_explosion_report(write_json(
            &dir,
            r#"{"version":"26.1.2","max_state_id":0,"entries":[0.0]}"#,
        ))
        .unwrap_err();

        assert!(matches!(
            error,
            BlockExplosionError::UnexpectedMaxStateId {
                actual: 0,
                expected: EXPECTED_MAX_STATE_ID
            }
        ));
    }

    #[test]
    fn rejects_negative_resistance_with_state_id_and_value() {
        let dir = TempDir::new().unwrap();
        let error = load_block_explosion_report(write_exact_json(&dir, &[(1, -0.25)])).unwrap_err();

        assert!(matches!(
            error,
            BlockExplosionError::InvalidResistance {
                state_id: 1,
                value: -0.25
            }
        ));
    }

    #[test]
    fn rejects_f32_overflow_with_state_id_and_value() {
        let dir = TempDir::new().unwrap();
        let error = load_block_explosion_report(write_exact_json(&dir, &[(0, 1e100)])).unwrap_err();

        assert!(matches!(
            error,
            BlockExplosionError::InvalidResistance {
                state_id: 0,
                value
            } if value == 1e100
        ));
    }

    #[test]
    fn distinguishes_missing_io_and_parse_errors() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.json");
        assert!(matches!(
            load_block_explosion_report(&missing).unwrap_err(),
            BlockExplosionError::Missing(path) if path == missing
        ));

        let directory = dir.path().join("directory");
        fs::create_dir(&directory).unwrap();
        assert!(matches!(
            load_block_explosion_report(&directory).unwrap_err(),
            BlockExplosionError::Io { .. }
        ));

        let malformed = write_json(&dir, "not-json");
        assert!(matches!(
            load_block_explosion_report(malformed).unwrap_err(),
            BlockExplosionError::Parse { .. }
        ));
    }

    #[test]
    fn real_2612_sidecar_has_expected_cardinal_resistances() {
        let reports = workspace_path("data/vanilla/reports");
        let explosion_path = reports.join("block_explosion.json");
        let blocks_path = reports.join("blocks.json");
        if !explosion_path.is_file() || !blocks_path.is_file() {
            eprintln!("skipping: run tools/extract-vanilla-data.sh for the real sidecar test");
            return;
        }

        let table = load_block_explosion_report(explosion_path).unwrap();
        let blocks = crate::blocks::load_blocks_report(blocks_path).unwrap();
        assert_eq!(table.len(), 29_873);

        let default_state = |name: &str| {
            blocks
                .iter()
                .find(|block| block.id.as_str() == name)
                .and_then(|block| block.states.iter().find(|state| state.default))
                .unwrap_or_else(|| panic!("missing default state for {name}"))
                .id
        };
        for (name, expected) in [
            ("minecraft:air", 0.0),
            ("minecraft:stone", 6.0),
            ("minecraft:dirt", 0.5),
            ("minecraft:bedrock", 3_600_000.0),
            ("minecraft:obsidian", 1_200.0),
            ("minecraft:water", 100.0),
            ("minecraft:lava", 100.0),
        ] {
            assert_eq!(
                table.resistance(default_state(name)),
                Some(expected),
                "{name}"
            );
        }

        let waterlogged_oak_slab = blocks
            .iter()
            .find(|block| block.id.as_str() == "minecraft:oak_slab")
            .and_then(|block| {
                block.states.iter().find(|state| {
                    state
                        .properties
                        .get("waterlogged")
                        .is_some_and(|value| value == "true")
                })
            })
            .expect("missing minecraft:oak_slab[waterlogged=true]");
        assert_eq!(
            table.resistance(waterlogged_oak_slab.id),
            Some(100.0),
            "waterlogged oak slab must include water resistance"
        );
    }
}
