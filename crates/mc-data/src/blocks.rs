//! Loader for the vanilla `blocks.json` report produced by
//! `server.jar --reports` (see `tools/extract-vanilla-data.sh`).
//!
//! Returns plain data — the typed `BlockRegistry` is built on top in
//! `mc-world`. Keeping the parser here means `mc-world` does not need
//! `serde`/`serde_json` or any filesystem awareness.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

const REQUIRED_BLOCKS: &str = include_str!("../data/required_blocks.json");

/// One block entry from `blocks.json`.
///
/// Property schema (`properties`) lists each property's allowed values
/// in the order the report uses; the report's BTreeMap iteration
/// order is what we capture. Individual states' property values
/// (`BlockStateReport::properties`) are stored as a sorted map; the
/// caller is expected to normalise lookup order against the schema.
#[derive(Debug, Clone)]
pub struct BlockReport {
    pub id: Identifier,
    pub properties: BTreeMap<String, Vec<String>>,
    pub states: Vec<BlockStateReport>,
}

#[derive(Debug, Clone)]
pub struct BlockStateReport {
    pub id: u32,
    pub default: bool,
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum BlocksReportError {
    #[error("blocks.json not found at {0}")]
    Missing(PathBuf),
    #[error("blocks.json io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocks.json parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("invalid block identifier {0:?} in blocks.json")]
    InvalidIdentifier(String),
}

/// Read and parse the report at `path`. Returns one entry per block
/// in the order they appear in the file's JSON object (preserved by
/// BTreeMap key ordering — alphabetical).
pub fn load_blocks_report(path: impl AsRef<Path>) -> Result<Vec<BlockReport>, BlocksReportError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(BlocksReportError::Missing(path.to_path_buf()));
    }
    let bytes = std::fs::read(path)?;
    let raw: BTreeMap<String, RawBlock> = serde_json::from_slice(&bytes)?;
    raw_blocks_to_report(raw)
}

/// Embedded derived block-state registry used when the local sidecar is absent.
#[must_use]
pub fn solaris_required_blocks_report() -> Vec<BlockReport> {
    let raw: BTreeMap<String, RawBlock> = serde_json::from_str(REQUIRED_BLOCKS)
        .expect("embedded required block registry JSON is valid");
    raw_blocks_to_report(raw).expect("embedded required block ids are valid")
}

fn raw_blocks_to_report(
    raw: BTreeMap<String, RawBlock>,
) -> Result<Vec<BlockReport>, BlocksReportError> {
    raw.into_iter()
        .map(|(name, body)| {
            let id = Identifier::parse(name.clone())
                .map_err(|_| BlocksReportError::InvalidIdentifier(name))?;
            Ok(BlockReport {
                id,
                properties: body.properties.unwrap_or_default(),
                states: body
                    .states
                    .into_iter()
                    .map(|s| BlockStateReport {
                        id: s.id,
                        default: s.default.unwrap_or(false),
                        properties: s.properties.unwrap_or_default(),
                    })
                    .collect(),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct RawBlock {
    #[serde(default)]
    properties: Option<BTreeMap<String, Vec<String>>>,
    states: Vec<RawState>,
}

#[derive(Deserialize)]
struct RawState {
    id: u32,
    #[serde(default)]
    default: Option<bool>,
    #[serde(default)]
    properties: Option<BTreeMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_path(rel: &str) -> PathBuf {
        // CARGO_MANIFEST_DIR is .../crates/mc-data; workspace root is two up.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    #[test]
    fn parses_minimal_inline() {
        let json = r#"{
            "minecraft:air": {
                "definition": { "type": "minecraft:air" },
                "states": [{ "default": true, "id": 0 }]
            },
            "minecraft:oak_log": {
                "definition": { "type": "minecraft:rotated_pillar" },
                "properties": { "axis": ["x", "y", "z"] },
                "states": [
                    { "id": 136, "properties": { "axis": "x" } },
                    { "default": true, "id": 137, "properties": { "axis": "y" } },
                    { "id": 138, "properties": { "axis": "z" } }
                ]
            }
        }"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), json).unwrap();
        let r = load_blocks_report(tmp.path()).unwrap();
        assert_eq!(r.len(), 2);
        let air = r.iter().find(|b| b.id.as_str() == "minecraft:air").unwrap();
        assert_eq!(air.states[0].id, 0);
        assert!(air.states[0].default);
        let oak = r
            .iter()
            .find(|b| b.id.as_str() == "minecraft:oak_log")
            .unwrap();
        assert_eq!(oak.properties["axis"], vec!["x", "y", "z"]);
        let default = oak.states.iter().find(|s| s.default).unwrap();
        assert_eq!(default.id, 137);
        assert_eq!(default.properties["axis"], "y");
    }

    #[test]
    fn missing_file_errors() {
        let err = load_blocks_report("/nonexistent/blocks.json").unwrap_err();
        assert!(matches!(err, BlocksReportError::Missing(_)));
    }

    #[test]
    fn embedded_required_blocks_cover_dense_26_1_state_registry() {
        let report = solaris_required_blocks_report();

        assert_eq!(report.len(), 1168, "block count for 26.1.2");
        let total_states: usize = report.iter().map(|b| b.states.len()).sum();
        assert_eq!(total_states, 29873, "total block states for 26.1.2");
        let air = report
            .iter()
            .find(|b| b.id.as_str() == "minecraft:air")
            .unwrap();
        assert_eq!(air.states[0].id, 0);
        let birch = report
            .iter()
            .find(|b| b.id.as_str() == "minecraft:birch_log")
            .unwrap();
        assert!(
            birch
                .states
                .iter()
                .any(|state| state.default && state.id == 143)
        );
    }

    /// Smoke test against the real vanilla report when one is present.
    /// Skips silently in CI / fresh checkouts where the sidecar is
    /// absent. The numbers are pinned to 26.1.2; update if the bundled
    /// server.jar is upgraded.
    #[test]
    fn loads_real_blocks_report_if_present() {
        let path = workspace_path("data/vanilla/reports/blocks.json");
        if !path.is_file() {
            eprintln!(
                "skipping: {} not present (run tools/extract-vanilla-data.sh)",
                path.display()
            );
            return;
        }
        let report = load_blocks_report(&path).unwrap();
        assert_eq!(report.len(), 1168, "block count for 26.1.2");
        let total_states: usize = report.iter().map(|b| b.states.len()).sum();
        assert_eq!(total_states, 29873, "total block states for 26.1.2");
        let air = report
            .iter()
            .find(|b| b.id.as_str() == "minecraft:air")
            .unwrap();
        assert_eq!(air.states[0].id, 0);
        let stone = report
            .iter()
            .find(|b| b.id.as_str() == "minecraft:stone")
            .unwrap();
        assert_eq!(stone.states[0].id, 1);
    }
}
