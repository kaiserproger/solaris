//! Per-block-state light metadata. When a vanilla sidecar is configured,
//! Solaris loads `data/vanilla/reports/block_light.json` as a required
//! runtime oracle. Without a sidecar, local fallback mode builds a
//! conservative table from the embedded blocks report.
//!
//! - `emission` — `BlockState.getLightEmission()`, 0..=15 (luminance
//!   the block radiates; torches=14, glowstone=15, soul_lantern=10).
//! - `opacity` — `BlockState.getLightDampening()`, 0..=15 (loss per
//!   step when light passes through this cell; vanilla mostly uses 0
//!   for fully transparent, 15 for fully opaque, and 1 for "soft"
//!   attenuators like water/ice/leaves).
//! - `propagates_sky` — `BlockState.propagatesSkylightDown()`, the
//!   predicate that drives sky-light's heightmap shortcut.
//! - `suffocating` — `BlockState.isSuffocating()`, used by gameplay
//!   checks that require a full blocking collision shape.
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

use crate::{blocks::BlockReport, read_json_file};

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
         expected 4 ([emission, opacity, propagates_sky, suffocating]); \
         rerun tools/extract-block-light.sh"
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
    #[error("entry at state-id {state_id} has suffocating = {value}; must be 0 or 1")]
    InvalidSuffocating { state_id: u32, value: i64 },
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
    suffocating: Vec<bool>,
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

    #[must_use]
    pub fn suffocating(&self, state_id: u32) -> Option<bool> {
        self.suffocating.get(state_id as usize).copied()
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
        let suffocating = opacity.iter().map(|opacity| *opacity == 15).collect();
        Self {
            version: version.into(),
            emission,
            opacity,
            propagates_sky,
            suffocating,
        }
    }

    #[must_use]
    pub fn from_arrays_with_suffocating(
        version: impl Into<String>,
        emission: Vec<u8>,
        opacity: Vec<u8>,
        propagates_sky: Vec<bool>,
        suffocating: Vec<bool>,
    ) -> Self {
        assert_eq!(emission.len(), opacity.len());
        assert_eq!(emission.len(), propagates_sky.len());
        assert_eq!(emission.len(), suffocating.len());
        Self {
            version: version.into(),
            emission,
            opacity,
            propagates_sky,
            suffocating,
        }
    }

    /// Append one full, opaque, non-emitting block state after a validated
    /// frozen registry.
    pub fn append_opaque_state(&mut self) {
        self.emission.push(0);
        self.opacity.push(15);
        self.propagates_sky.push(false);
        self.suffocating.push(true);
    }

    /// Build a runtime table from `blocks.json` when the exact vanilla
    /// light oracle is absent. Unknown blocks are treated as opaque and
    /// non-emissive, which keeps caves and terrain sane; known air,
    /// transparent, and light-emitting block families get small explicit
    /// overrides.
    #[must_use]
    pub fn conservative_from_blocks_report(report: &[BlockReport]) -> Self {
        let len = report
            .iter()
            .flat_map(|block| block.states.iter().map(|state| state.id as usize))
            .max()
            .map_or(0, |id| id + 1);
        let mut emission = vec![0; len];
        let mut opacity = vec![15; len];
        let mut propagates_sky = vec![false; len];
        let mut suffocating = vec![false; len];

        for block in report {
            let path = block.id.path();
            for state in &block.states {
                let idx = state.id as usize;
                emission[idx] = conservative_emission(path, &state.properties);
                let op = conservative_opacity(path);
                opacity[idx] = op;
                propagates_sky[idx] = op == 0;
                suffocating[idx] = op == 15;
            }
        }

        Self {
            version: "blocks-report-conservative".to_string(),
            emission,
            opacity,
            propagates_sky,
            suffocating,
        }
    }
}

fn conservative_opacity(path: &str) -> u8 {
    if matches!(
        path,
        "kelp" | "kelp_plant" | "chorus_flower" | "chorus_plant"
    ) {
        return 1;
    }

    if matches!(path, "air" | "cave_air" | "void_air")
        || path.contains("glass")
        || path.contains("pane")
        || path.contains("water")
        || path.contains("ice")
        || path.contains("leaves")
        || path.ends_with("torch")
        || matches!(path, "lantern" | "soul_lantern")
        || is_transparent_plant(path)
        || is_transparent_crop(path)
        || path == "sugar_cane"
        || path == "cactus"
        || path.contains("flower")
        || path.contains("sapling")
        || path.contains("mushroom")
        || matches!(
            path,
            "short_grass" | "tall_grass" | "seagrass" | "tall_seagrass"
        )
        || path.contains("fern")
        || path.contains("vine")
        || path.contains("roots")
        || path.contains("bush")
        || path.contains("carpet")
        || path.contains("rail")
        || path.contains("sign")
        || path.contains("button")
        || path.contains("pressure_plate")
        || path.contains("ladder")
        || path.contains("chain")
        || path.contains("candle")
        || path.contains("fire")
    {
        0
    } else {
        15
    }
}

fn is_transparent_crop(path: &str) -> bool {
    matches!(
        path,
        "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "torchflower_crop"
            | "pitcher_crop"
            | "melon_stem"
            | "attached_melon_stem"
            | "pumpkin_stem"
            | "attached_pumpkin_stem"
            | "sweet_berry_bush"
            | "nether_wart"
            | "cocoa"
            | "bamboo"
    )
}

fn is_transparent_plant(path: &str) -> bool {
    matches!(
        path,
        "poppy"
            | "dandelion"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "red_tulip"
            | "orange_tulip"
            | "white_tulip"
            | "pink_tulip"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "wither_rose"
            | "open_eyeblossom"
            | "closed_eyeblossom"
            | "torchflower"
            | "pitcher_plant"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "sunflower"
    )
}

fn conservative_emission(path: &str, props: &std::collections::BTreeMap<String, String>) -> u8 {
    if props.get("lit").is_some_and(|v| v == "false") {
        return 0;
    }
    if matches!(path, "torchflower" | "torchflower_crop") {
        return 0;
    }
    if matches!(path, "cave_vines" | "cave_vines_plant")
        && props.get("berries").is_some_and(|value| value == "true")
    {
        return 14;
    }
    if path.contains("redstone_torch") {
        return 7;
    }
    if path.contains("soul_torch") || path.contains("soul_lantern") || path.contains("soul_fire") {
        return 10;
    }
    if path.contains("torch") {
        return 14;
    }
    if path.contains("glowstone")
        || path.contains("sea_lantern")
        || path.contains("jack_o_lantern")
        || path.contains("redstone_lamp")
        || path == "lava"
        || path.contains("lava")
        || path.contains("fire")
        || path.contains("lantern")
    {
        return 15;
    }
    if path.contains("candle") {
        return props
            .get("candles")
            .and_then(|v| v.parse::<u8>().ok())
            .map_or(3, |n| n.saturating_mul(3).min(12));
    }
    0
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
    let raw: RawTable = read_json_file(
        path,
        &|path, source| BlockLightError::Io { path, source },
        &|path, source| BlockLightError::Parse { path, source },
    )?;

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
    let mut suffocating = Vec::with_capacity(expected);
    for (state_id, e) in raw.entries.iter().enumerate() {
        if e.len() != 4 {
            return Err(BlockLightError::EntryShape {
                state_id: state_id as u32,
                got: e.len(),
            });
        }
        let em = e[0];
        let op = e[1];
        let ps = e[2];
        let sf = e[3];
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
        if !(0..=1).contains(&sf) {
            return Err(BlockLightError::InvalidSuffocating {
                state_id: state_id as u32,
                value: sf,
            });
        }
        emission.push(em as u8);
        opacity.push(op as u8);
        propagates_sky.push(ps == 1);
        suffocating.push(sf == 1);
    }

    Ok(BlockLightTable {
        version: raw.version,
        emission,
        opacity,
        propagates_sky,
        suffocating,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Identifier;
    use crate::blocks::{BlockReport, BlockStateReport};
    use std::collections::BTreeMap;
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

    fn block(id: u32, name: &str) -> BlockReport {
        BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id,
                default: true,
                properties: BTreeMap::new(),
            }],
        }
    }

    #[test]
    fn from_arrays_pins_lookups() {
        let mut table = BlockLightTable::from_arrays(
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
        assert_eq!(table.suffocating(2), Some(true));
        assert_eq!(table.emission(99), None);
        table.append_opaque_state();
        assert_eq!(table.emission(3), Some(0));
        assert_eq!(table.opacity(3), Some(15));
        assert_eq!(table.propagates_sky(3), Some(false));
        assert_eq!(table.suffocating(3), Some(true));
    }

    #[test]
    fn conservative_table_from_blocks_report_keeps_light_running_without_sidecar() {
        let table = BlockLightTable::conservative_from_blocks_report(&[
            block(0, "minecraft:air"),
            block(1, "minecraft:stone"),
            block(2, "minecraft:grass_block"),
            block(3, "minecraft:glass"),
            block(4, "minecraft:torch"),
            block(5, "minecraft:glowstone"),
            block(6, "minecraft:poppy"),
            block(7, "minecraft:sugar_cane"),
            block(8, "minecraft:wheat"),
        ]);

        assert_eq!(table.version, "blocks-report-conservative");
        assert_eq!(table.len(), 9);
        assert_eq!(table.opacity(0), Some(0));
        assert_eq!(table.propagates_sky(0), Some(true));
        assert_eq!(table.opacity(1), Some(15));
        assert_eq!(table.propagates_sky(1), Some(false));
        assert_eq!(
            table.opacity(2),
            Some(15),
            "grass_block is terrain, not grass"
        );
        assert_eq!(table.opacity(3), Some(0));
        assert_eq!(table.emission(4), Some(14));
        assert_eq!(table.emission(5), Some(15));
        assert_eq!(table.opacity(6), Some(0));
        assert_eq!(table.opacity(7), Some(0));
        assert_eq!(table.opacity(8), Some(0));
        assert_eq!(table.propagates_sky(8), Some(true));
    }

    #[test]
    fn conservative_table_matches_supported_crop_light_classes() {
        let full_sky = [
            "minecraft:wheat",
            "minecraft:carrots",
            "minecraft:potatoes",
            "minecraft:beetroots",
            "minecraft:torchflower_crop",
            "minecraft:torchflower",
            "minecraft:pitcher_crop",
            "minecraft:pitcher_plant",
            "minecraft:melon_stem",
            "minecraft:attached_melon_stem",
            "minecraft:pumpkin_stem",
            "minecraft:attached_pumpkin_stem",
            "minecraft:sweet_berry_bush",
            "minecraft:nether_wart",
            "minecraft:cocoa",
            "minecraft:cactus",
            "minecraft:sugar_cane",
            "minecraft:bamboo",
            "minecraft:bamboo_sapling",
        ];
        let soft_light = [
            "minecraft:kelp",
            "minecraft:kelp_plant",
            "minecraft:chorus_flower",
            "minecraft:chorus_plant",
        ];
        let reports = full_sky
            .iter()
            .chain(soft_light.iter())
            .enumerate()
            .map(|(id, name)| block(id as u32, name))
            .collect::<Vec<_>>();
        let table = BlockLightTable::conservative_from_blocks_report(&reports);

        for (id, name) in full_sky.iter().enumerate() {
            assert_eq!(table.emission(id as u32), Some(0), "{name}");
            assert_eq!(table.opacity(id as u32), Some(0), "{name}");
            assert_eq!(table.propagates_sky(id as u32), Some(true), "{name}");
        }
        for (offset, name) in soft_light.iter().enumerate() {
            let id = (full_sky.len() + offset) as u32;
            assert_eq!(table.emission(id), Some(0), "{name}");
            assert_eq!(table.opacity(id), Some(1), "{name}");
            assert_eq!(table.propagates_sky(id), Some(false), "{name}");
        }
    }

    #[test]
    fn conservative_table_matches_glow_berry_vine_emission() {
        let mut berries = BTreeMap::new();
        berries.insert("berries".to_string(), "true".to_string());
        let mut empty = BTreeMap::new();
        empty.insert("berries".to_string(), "false".to_string());

        assert_eq!(conservative_emission("cave_vines", &berries), 14);
        assert_eq!(conservative_emission("cave_vines", &empty), 0);
        assert_eq!(conservative_emission("cave_vines_plant", &berries), 14);
        assert_eq!(conservative_emission("cave_vines_plant", &empty), 0);
        assert_eq!(conservative_opacity("cave_vines"), 0);
        assert_eq!(conservative_opacity("cave_vines_plant"), 0);
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
                "entries": [[0,0,1,0],[14,0,1,0],[0,15,0,1]]
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
    fn loads_synthetic_table_with_suffocation_fact() {
        let dir = TempDir::new().unwrap();
        let path = write_json(
            &dir,
            "block_light.json",
            r#"{
                "version": "26.1.2-test",
                "max_state_id": 1,
                "entries": [[0,0,1,0],[0,15,0,1]]
            }"#,
        );

        let table = load(&path).expect("four-field block-state facts should load");
        assert_eq!(table.len(), 2);
        assert_eq!(table.suffocating(0), Some(false));
        assert_eq!(table.suffocating(1), Some(true));
    }

    #[test]
    fn three_field_table_requires_regeneration() {
        let dir = TempDir::new().unwrap();
        let path = write_json(
            &dir,
            "block_light.json",
            r#"{
                "version": "26.1.2-test",
                "max_state_id": 0,
                "entries": [[0,0,1]]
            }"#,
        );

        let error = load(&path).expect_err("old table must not guess suffocation from opacity");
        assert!(matches!(
            error,
            BlockLightError::EntryShape {
                state_id: 0,
                got: 3
            }
        ));
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
            r#"{"version":"v","max_state_id":0,"entries":[[16,0,1,0]]}"#,
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
            r#"{"version":"v","max_state_id":0,"entries":[[0,0,2,0]]}"#,
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
    #[ignore = "requires local 26.1.2 block light and blocks reports"]
    fn real_table_matches_known_blocks() {
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        let light_path = workspace_path("data/vanilla/reports/block_light.json");
        assert!(
            blocks_path.is_file() && light_path.is_file(),
            "need both {} and {}; run tools/extract-vanilla-data.sh",
            blocks_path.display(),
            light_path.display()
        );

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
        assert_eq!(table.suffocating(stone), Some(true));

        let slab = default_state("minecraft:oak_slab");
        assert_eq!(table.suffocating(slab), Some(false));

        let soul_sand = default_state("minecraft:soul_sand");
        assert_eq!(table.suffocating(soul_sand), Some(true));

        let barrier = default_state("minecraft:barrier");
        assert_eq!(table.opacity(barrier), Some(0));
        assert_eq!(table.suffocating(barrier), Some(true));

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
