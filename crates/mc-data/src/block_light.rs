//! Per-block-state light metadata loaded from
//! `data/vanilla/reports/block_light.json` (produced by
//! `tools/extract-block-light.sh`). Three fields per global block-state
//! id, copied verbatim from the unobfuscated jar's
//! `BlockBehaviour$BlockStateBase` public API:
//!
//! - `emission` — `BlockState.getLightEmission()`, 0..=15 (luminance
//!   the block radiates; torches=14, glowstone=15, soul_lantern=10).
//! - `opacity` — `BlockState.getLightDampening()`, 0..=15 (loss per
//!   step when light passes through this cell; vanilla mostly uses 0
//!   for fully transparent, 15 for fully opaque, and 1 for "soft"
//!   attenuators like water/ice/leaves).
//! - `propagates_sky` — `BlockState.propagatesSkylightDown()`, the
//!   predicate that drives sky-light's heightmap shortcut.
//!
//! The script's output is data (ADR 0001); the table is loaded at
//! server start with the same posture as `blocks.json`.
//!
//! See `tools/extract-block-light/LightExtractor.java` for the
//! extraction. The Rust side is intentionally schema-stable: any
//! incompatible change ships under a bumped `version` field, the
//! script regenerates the file, and the loader's mismatch error is
//! surfaced as a clear runtime message.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BlockLightError {
    #[error("block_light.json not found at {0}; did you run tools/extract-block-light.sh?")]
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
        "entry at state-id {state_id} has {got} elements; \
         expected 3 ([emission, opacity, propagates_sky])"
    )]
    EntryShape { state_id: u32, got: usize },
    #[error("entry at state-id {state_id} has {field} = {value}; must be in 0..=15")]
    OutOfRange {
        state_id: u32,
        field: &'static str,
        value: i64,
    },
    #[error("entry at state-id {state_id} has propagates_sky = {value}; must be 0 or 1")]
    InvalidBool { state_id: u32, value: i64 },
}

/// Per-state light metadata, indexed by global block-state id (the
/// same id space `mc_data::blocks::BlockStateReport::id` uses).
#[derive(Debug, Clone)]
pub struct BlockLightTable {
    /// Vanilla version string the table was extracted from
    /// (e.g. `"26.1.2"`). Surfaced in logs for sanity-checking that
    /// the table matches the `blocks.json` shipped alongside it.
    pub version: String,
    /// Length is `max_state_id + 1`; index = global state-id.
    emission: Vec<u8>,
    opacity: Vec<u8>,
    propagates_sky: Vec<bool>,
}

impl BlockLightTable {
    #[must_use]
    pub fn len(&self) -> usize {
        self.emission.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.emission.is_empty()
    }

    /// Emission (luminance) in `0..=15` for the given state-id, or
    /// `None` if the id is out of range.
    #[must_use]
    pub fn emission(&self, state_id: u32) -> Option<u8> {
        self.emission.get(state_id as usize).copied()
    }

    /// Opacity (`getLightDampening` in vanilla) in `0..=15` for the
    /// given state-id, or `None` if the id is out of range.
    #[must_use]
    pub fn opacity(&self, state_id: u32) -> Option<u8> {
        self.opacity.get(state_id as usize).copied()
    }

    /// Whether sky-light passes through this state.
    #[must_use]
    pub fn propagates_sky(&self, state_id: u32) -> Option<bool> {
        self.propagates_sky.get(state_id as usize).copied()
    }

    /// Build a table from in-memory arrays; used by tests and by
    /// callers that don't want to stage a filesystem layout.
    /// Panics if the three arrays have different lengths.
    #[must_use]
    pub fn from_arrays(
        version: impl Into<String>,
        emission: Vec<u8>,
        opacity: Vec<u8>,
        propagates_sky: Vec<bool>,
    ) -> Self {
        assert_eq!(emission.len(), opacity.len());
        assert_eq!(emission.len(), propagates_sky.len());
        Self {
            version: version.into(),
            emission,
            opacity,
            propagates_sky,
        }
    }
}

#[derive(Deserialize)]
struct RawTable {
    version: String,
    max_state_id: u32,
    entries: Vec<Vec<i64>>,
}

/// Read and parse the table at `path`.
pub fn load(path: impl AsRef<Path>) -> Result<BlockLightTable, BlockLightError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(BlockLightError::Missing(path.to_path_buf()));
    }
    let bytes = std::fs::read(path).map_err(|source| BlockLightError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let raw: RawTable =
        serde_json::from_slice(&bytes).map_err(|source| BlockLightError::Parse {
            path: path.to_path_buf(),
            source,
        })?;

    let expected = raw.max_state_id as usize + 1;
    if raw.entries.len() != expected {
        return Err(BlockLightError::LengthMismatch {
            entries: raw.entries.len(),
            max_state_id: raw.max_state_id,
            expected,
        });
    }

    let mut emission = Vec::with_capacity(expected);
    let mut opacity = Vec::with_capacity(expected);
    let mut propagates_sky = Vec::with_capacity(expected);
    for (state_id, e) in raw.entries.iter().enumerate() {
        if e.len() != 3 {
            return Err(BlockLightError::EntryShape {
                state_id: state_id as u32,
                got: e.len(),
            });
        }
        let em = e[0];
        let op = e[1];
        let ps = e[2];
        if !(0..=15).contains(&em) {
            return Err(BlockLightError::OutOfRange {
                state_id: state_id as u32,
                field: "emission",
                value: em,
            });
        }
        if !(0..=15).contains(&op) {
            return Err(BlockLightError::OutOfRange {
                state_id: state_id as u32,
                field: "opacity",
                value: op,
            });
        }
        if !(0..=1).contains(&ps) {
            return Err(BlockLightError::InvalidBool {
                state_id: state_id as u32,
                value: ps,
            });
        }
        emission.push(em as u8);
        opacity.push(op as u8);
        propagates_sky.push(ps == 1);
    }

    Ok(BlockLightTable {
        version: raw.version,
        emission,
        opacity,
        propagates_sky,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    fn write_json(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn from_arrays_pins_lookups() {
        let table = BlockLightTable::from_arrays(
            "test",
            vec![0, 14, 15],
            vec![0, 0, 15],
            vec![true, true, false],
        );
        assert_eq!(table.len(), 3);
        assert!(!table.is_empty());
        assert_eq!(table.emission(0), Some(0));
        assert_eq!(table.opacity(2), Some(15));
        assert_eq!(table.propagates_sky(1), Some(true));
        assert_eq!(table.emission(99), None);
    }

    #[test]
    fn loads_synthetic_table() {
        let dir = TempDir::new().unwrap();
        let path = write_json(
            &dir,
            "block_light.json",
            r#"{
                "version": "26.1.2-test",
                "max_state_id": 2,
                "entries": [[0,0,1],[14,0,1],[0,15,0]]
            }"#,
        );
        let t = load(&path).unwrap();
        assert_eq!(t.version, "26.1.2-test");
        assert_eq!(t.len(), 3);
        assert_eq!(t.emission(1), Some(14));
        assert_eq!(t.opacity(2), Some(15));
        assert_eq!(t.propagates_sky(2), Some(false));
    }

    #[test]
    fn missing_file_is_reported_clearly() {
        let err = load("/definitely/does/not/exist/block_light.json").unwrap_err();
        assert!(matches!(err, BlockLightError::Missing(_)));
    }

    #[test]
    fn length_mismatch_is_an_error() {
        let dir = TempDir::new().unwrap();
        let path = write_json(
            &dir,
            "x.json",
            r#"{"version":"v","max_state_id":1,"entries":[[0,0,1]]}"#,
        );
        let err = load(&path).unwrap_err();
        assert!(matches!(err, BlockLightError::LengthMismatch { .. }));
    }

    #[test]
    fn out_of_range_emission_is_an_error() {
        let dir = TempDir::new().unwrap();
        let path = write_json(
            &dir,
            "x.json",
            r#"{"version":"v","max_state_id":0,"entries":[[16,0,1]]}"#,
        );
        let err = load(&path).unwrap_err();
        assert!(matches!(
            err,
            BlockLightError::OutOfRange {
                field: "emission",
                value: 16,
                ..
            }
        ));
    }

    #[test]
    fn invalid_propagates_sky_is_an_error() {
        let dir = TempDir::new().unwrap();
        let path = write_json(
            &dir,
            "x.json",
            r#"{"version":"v","max_state_id":0,"entries":[[0,0,2]]}"#,
        );
        let err = load(&path).unwrap_err();
        assert!(matches!(err, BlockLightError::InvalidBool { value: 2, .. }));
    }

    #[test]
    fn entry_shape_is_an_error() {
        let dir = TempDir::new().unwrap();
        let path = write_json(
            &dir,
            "x.json",
            r#"{"version":"v","max_state_id":0,"entries":[[0,0]]}"#,
        );
        let err = load(&path).unwrap_err();
        assert!(matches!(
            err,
            BlockLightError::EntryShape {
                state_id: 0,
                got: 2
            }
        ));
    }

    /// Pin a handful of values against the real extracted JSON when
    /// it is present. The exact state-ids are resolved through
    /// `blocks.json` so the test survives state-id reshuffles
    /// between Mojang patches.
    #[test]
    fn real_table_matches_known_blocks() {
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        let light_path = workspace_path("data/vanilla/reports/block_light.json");
        if !blocks_path.is_file() || !light_path.is_file() {
            eprintln!(
                "skipping: need both {} and {}",
                blocks_path.display(),
                light_path.display(),
            );
            return;
        }

        let blocks = crate::blocks::load_blocks_report(&blocks_path).unwrap();
        let table = load(&light_path).unwrap();
        assert_eq!(
            table.version, "26.1.2",
            "block_light.json version should track 26.1.2",
        );
        assert_eq!(table.len(), 29873, "26.1.2 has 29873 block states");

        let default_state = |name: &str| -> u32 {
            blocks
                .iter()
                .find(|b| b.id.as_str() == name)
                .unwrap_or_else(|| panic!("missing {name}"))
                .states
                .iter()
                .find(|s| s.default)
                .unwrap_or_else(|| panic!("{name} has no default state"))
                .id
        };

        // air: transparent, no emission, sky passes through.
        let air = default_state("minecraft:air");
        assert_eq!(table.emission(air), Some(0));
        assert_eq!(table.opacity(air), Some(0));
        assert_eq!(table.propagates_sky(air), Some(true));

        // stone: opaque, no emission, blocks sky.
        let stone = default_state("minecraft:stone");
        assert_eq!(table.emission(stone), Some(0));
        assert_eq!(table.opacity(stone), Some(15));
        assert_eq!(table.propagates_sky(stone), Some(false));

        // glowstone: full luminance (15).
        let glowstone = default_state("minecraft:glowstone");
        assert_eq!(table.emission(glowstone), Some(15));

        // wall_torch: torches sit at emission 14.
        let torch = default_state("minecraft:torch");
        assert_eq!(table.emission(torch), Some(14));

        // soul_lantern: 10 in 26.1.2 (vanilla-known).
        let soul_lantern = default_state("minecraft:soul_lantern");
        assert_eq!(table.emission(soul_lantern), Some(10));

        // water: opacity 1 (soft attenuator), no emission, sky still
        // passes through (water doesn't fully block sky-light).
        let water = default_state("minecraft:water");
        assert_eq!(table.emission(water), Some(0));
        assert_eq!(table.opacity(water), Some(1));
    }
}
