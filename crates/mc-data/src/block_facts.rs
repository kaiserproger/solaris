//! Narrow block-behaviour facts derived from the block report.

use crate::Identifier;
use crate::block_explosion::BlockExplosionTable;
use crate::block_mining::{BlockMiningFacts, BlockMiningTable};
use crate::blocks::BlockReport;
use crate::collision_shapes::{COLLISION_UNITS_PER_BLOCK, vanilla_collision_shapes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomTickFamily {
    Crop,
    Farmland,
    Fire,
    Grass,
    Leaves,
    Sapling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluidKind {
    Water,
    Lava,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SturdyFace {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidStateFacts {
    pub kind: FluidKind,
    pub level: u8,
    pub source: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockFacts {
    pub random_tick_family: Option<RandomTickFamily>,
    pub fluid: Option<FluidStateFacts>,
}

#[derive(Debug, Clone, Default)]
pub struct BlockFactsTable {
    states: Vec<BlockFacts>,
    mining: Vec<Option<BlockMiningFacts>>,
    explosion: Option<BlockExplosionTable>,
    eligible_states: usize,
}

impl BlockFactsTable {
    #[must_use]
    pub fn from_blocks_report(report: &[BlockReport]) -> Self {
        Self::from_blocks_report_with_mining(report, None)
    }

    #[must_use]
    pub fn from_blocks_report_with_mining(
        report: &[BlockReport],
        mining_table: Option<&BlockMiningTable>,
    ) -> Self {
        let max_state = report
            .iter()
            .flat_map(|block| block.states.iter().map(|state| state.id as usize))
            .max()
            .unwrap_or(0);
        let mut states = vec![BlockFacts::default(); max_state.saturating_add(1)];
        let mut mining = vec![None; states.len()];
        let mut eligible_states = 0;
        for block in report {
            let family = classify_random_tick_family(block.id.path());
            let fluid_kind = classify_fluid_kind(block.id.path());
            for state in &block.states {
                let facts = &mut states[state.id as usize];
                if facts.random_tick_family.is_none() && family.is_some() {
                    eligible_states += 1;
                }
                facts.random_tick_family = family;
                facts.fluid = fluid_kind.and_then(|kind| {
                    state
                        .properties
                        .get("level")
                        .and_then(|level| level.parse::<u8>().ok())
                        .map(|level| FluidStateFacts {
                            kind,
                            level,
                            source: level == 0,
                        })
                });
                mining[state.id as usize] = mining_table.and_then(|table| table.facts(state.id));
            }
        }
        Self {
            states,
            mining,
            explosion: None,
            eligible_states,
        }
    }

    #[must_use]
    pub fn with_explosion_table(mut self, explosion: BlockExplosionTable) -> Self {
        self.explosion = Some(explosion);
        self
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.states.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    #[must_use]
    pub fn eligible_states(&self) -> usize {
        self.eligible_states
    }

    #[must_use]
    pub fn random_tick_family(&self, state_id: u32) -> Option<RandomTickFamily> {
        self.states
            .get(state_id as usize)
            .and_then(|facts| facts.random_tick_family)
    }

    #[must_use]
    pub fn fluid(&self, state_id: u32) -> Option<FluidStateFacts> {
        self.states
            .get(state_id as usize)
            .and_then(|facts| facts.fluid)
    }

    #[must_use]
    pub fn mining(&self, state_id: u32) -> Option<BlockMiningFacts> {
        self.mining.get(state_id as usize).copied().flatten()
    }

    #[must_use]
    pub fn explosion_resistance(&self, state_id: u32) -> Option<f32> {
        self.explosion
            .as_ref()
            .and_then(|table| table.resistance(state_id))
    }

    #[must_use]
    pub fn has_explosion_table(&self) -> bool {
        self.explosion.is_some()
    }
}

/// Returns whether the exact embedded 26.1.2 state exposes a completely covered
/// collision face. A missing or fingerprint-mismatched state is deliberately
/// non-sturdy; block names are not sufficient evidence for support semantics.
#[must_use]
pub fn has_full_sturdy_face(
    state_id: u32,
    block: &Identifier,
    properties: &[(String, String)],
    face: SturdyFace,
) -> bool {
    let Some(shape) = vanilla_collision_shapes().get_for_state(state_id, block, properties) else {
        return false;
    };
    let mut covered = [false; 16 * 16];
    const CELL_UNITS: i16 = COLLISION_UNITS_PER_BLOCK / 16;

    for collision_box in shape.iter() {
        let [min_x, min_y, min_z, max_x, max_y, max_z] = collision_box.coordinates();
        let projected = match face {
            SturdyFace::Down if min_y == 0 => Some((min_x, max_x, min_z, max_z)),
            SturdyFace::Up if max_y == COLLISION_UNITS_PER_BLOCK => {
                Some((min_x, max_x, min_z, max_z))
            }
            SturdyFace::North if min_z == 0 => Some((min_x, max_x, min_y, max_y)),
            SturdyFace::South if max_z == COLLISION_UNITS_PER_BLOCK => {
                Some((min_x, max_x, min_y, max_y))
            }
            SturdyFace::West if min_x == 0 => Some((min_z, max_z, min_y, max_y)),
            SturdyFace::East if max_x == COLLISION_UNITS_PER_BLOCK => {
                Some((min_z, max_z, min_y, max_y))
            }
            SturdyFace::Down
            | SturdyFace::Up
            | SturdyFace::North
            | SturdyFace::South
            | SturdyFace::West
            | SturdyFace::East => None,
        };
        let Some((min_a, max_a, min_b, max_b)) = projected else {
            continue;
        };
        for a in 0..16 {
            for b in 0..16 {
                let cell_min_a = a * CELL_UNITS;
                let cell_max_a = cell_min_a + CELL_UNITS;
                let cell_min_b = b * CELL_UNITS;
                let cell_max_b = cell_min_b + CELL_UNITS;
                if min_a <= cell_min_a
                    && max_a >= cell_max_a
                    && min_b <= cell_min_b
                    && max_b >= cell_max_b
                {
                    covered[a as usize * 16 + b as usize] = true;
                }
            }
        }
    }

    covered.into_iter().all(|cell| cell)
}

fn classify_fluid_kind(path: &str) -> Option<FluidKind> {
    match path {
        "water" => Some(FluidKind::Water),
        "lava" => Some(FluidKind::Lava),
        _ => None,
    }
}

fn classify_random_tick_family(path: &str) -> Option<RandomTickFamily> {
    if path == "farmland" {
        return Some(RandomTickFamily::Farmland);
    }
    if path == "fire" || path == "soul_fire" {
        return Some(RandomTickFamily::Fire);
    }
    if path == "grass_block" {
        return Some(RandomTickFamily::Grass);
    }
    if path.ends_with("_leaves") || path == "azalea_leaves" || path == "flowering_azalea_leaves" {
        return Some(RandomTickFamily::Leaves);
    }
    if path.ends_with("_sapling") || path == "bamboo_sapling" {
        return Some(RandomTickFamily::Sapling);
    }
    if matches!(
        path,
        "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "melon_stem"
            | "pumpkin_stem"
            | "attached_melon_stem"
            | "attached_pumpkin_stem"
            | "sweet_berry_bush"
            | "cocoa"
            | "nether_wart"
            | "cactus"
            | "sugar_cane"
            | "bamboo"
            | "kelp"
            | "kelp_plant"
            | "chorus_flower"
            | "chorus_plant"
    ) {
        return Some(RandomTickFamily::Crop);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Identifier;
    use crate::blocks::{BlockReport, BlockStateReport};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
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

    fn fluid_block(first_id: u32, name: &str) -> BlockReport {
        let mut properties = BTreeMap::new();
        properties.insert("level".to_string(), vec!["0".to_string(), "1".to_string()]);

        BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties,
            states: [0, 1]
                .into_iter()
                .map(|level| {
                    let mut state_properties = BTreeMap::new();
                    state_properties.insert("level".to_string(), level.to_string());
                    BlockStateReport {
                        id: first_id + level,
                        default: level == 0,
                        properties: state_properties,
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn classifies_common_random_tick_families() {
        let table = BlockFactsTable::from_blocks_report(&[
            block(0, "minecraft:air"),
            block(1, "minecraft:stone"),
            block(2, "minecraft:wheat"),
            block(3, "minecraft:oak_leaves"),
            block(4, "minecraft:grass_block"),
            block(5, "minecraft:fire"),
            block(6, "minecraft:farmland"),
            block(7, "minecraft:oak_sapling"),
        ]);

        assert_eq!(table.random_tick_family(1), None);
        assert_eq!(table.random_tick_family(2), Some(RandomTickFamily::Crop));
        assert_eq!(table.random_tick_family(3), Some(RandomTickFamily::Leaves));
        assert_eq!(table.random_tick_family(4), Some(RandomTickFamily::Grass));
        assert_eq!(table.random_tick_family(5), Some(RandomTickFamily::Fire));
        assert_eq!(
            table.random_tick_family(6),
            Some(RandomTickFamily::Farmland)
        );
        assert_eq!(table.random_tick_family(7), Some(RandomTickFamily::Sapling));
        assert_eq!(table.eligible_states(), 6);
    }

    #[test]
    #[ignore = "requires local 26.1.2 blocks report"]
    fn loads_real_random_tick_families_when_present() {
        let path = workspace_path("data/vanilla/reports/blocks.json");
        assert!(
            path.is_file(),
            "{} not present; run tools/extract-vanilla-data.sh",
            path.display()
        );
        let report = crate::blocks::load_blocks_report(&path).unwrap();
        let table = BlockFactsTable::from_blocks_report(&report);
        assert!(table.eligible_states() > 0);
        let wheat = report
            .iter()
            .find(|block| block.id.as_str() == "minecraft:wheat")
            .unwrap();
        let wheat_default = wheat.states.iter().find(|state| state.default).unwrap();
        assert_eq!(
            table.random_tick_family(wheat_default.id),
            Some(RandomTickFamily::Crop)
        );
    }

    #[test]
    fn derives_fluid_facts_from_level_property() {
        let table = BlockFactsTable::from_blocks_report(&[
            block(0, "minecraft:air"),
            fluid_block(1, "minecraft:water"),
            fluid_block(3, "minecraft:lava"),
        ]);

        assert_eq!(
            table.fluid(1),
            Some(FluidStateFacts {
                kind: FluidKind::Water,
                level: 0,
                source: true,
            })
        );
        assert_eq!(
            table.fluid(2),
            Some(FluidStateFacts {
                kind: FluidKind::Water,
                level: 1,
                source: false,
            })
        );
        assert_eq!(
            table.fluid(3),
            Some(FluidStateFacts {
                kind: FluidKind::Lava,
                level: 0,
                source: true,
            })
        );
        assert_eq!(table.fluid(0), None);
    }

    #[test]
    fn attaches_exact_mining_facts_by_state_id() {
        let reports = [
            block(0, "minecraft:air"),
            block(1, "minecraft:stone"),
            block(2, "minecraft:oak_log"),
        ];
        let mining =
            BlockMiningTable::from_arrays("test", vec![0.0, 1.5, 2.0], vec![false, true, false]);

        let table = BlockFactsTable::from_blocks_report_with_mining(&reports, Some(&mining));

        assert_eq!(
            table.mining(1),
            Some(BlockMiningFacts {
                destroy_speed: 1.5,
                requires_correct_tool_for_drops: true,
            })
        );
        assert_eq!(table.mining(3), None);
    }

    #[test]
    #[ignore = "requires local 26.1.2 blocks report"]
    fn loads_real_fluid_facts_when_present() {
        let path = workspace_path("data/vanilla/reports/blocks.json");
        assert!(
            path.is_file(),
            "{} not present; run tools/extract-vanilla-data.sh",
            path.display()
        );
        let report = crate::blocks::load_blocks_report(&path).unwrap();
        let table = BlockFactsTable::from_blocks_report(&report);
        let water = report
            .iter()
            .find(|block| block.id.as_str() == "minecraft:water")
            .unwrap();
        let source = water
            .states
            .iter()
            .find(|state| {
                state
                    .properties
                    .get("level")
                    .is_some_and(|level| level == "0")
            })
            .unwrap();

        assert_eq!(
            table.fluid(source.id),
            Some(FluidStateFacts {
                kind: FluidKind::Water,
                level: 0,
                source: true,
            })
        );
    }

    #[test]
    fn exact_sturdy_faces_cover_full_blocks_and_irregular_common_supports() {
        let report = crate::blocks::solaris_required_blocks_report();
        let state = |name: &str, expected: &[(&str, &str)]| {
            let block = report
                .iter()
                .find(|block| block.id.as_str() == name)
                .unwrap_or_else(|| panic!("missing embedded block {name}"));
            let state = block
                .states
                .iter()
                .find(|state| {
                    expected.iter().all(|(key, value)| {
                        state
                            .properties
                            .get(*key)
                            .is_some_and(|actual| actual == value)
                    })
                })
                .unwrap_or_else(|| panic!("missing embedded state {name} {expected:?}"));
            let properties = state
                .properties
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>();
            (state.id, block.id.clone(), properties)
        };

        let (stone_id, stone, stone_properties) = state("minecraft:stone", &[]);
        for face in [
            SturdyFace::Down,
            SturdyFace::Up,
            SturdyFace::North,
            SturdyFace::South,
            SturdyFace::West,
            SturdyFace::East,
        ] {
            assert!(has_full_sturdy_face(
                stone_id,
                &stone,
                &stone_properties,
                face
            ));
        }

        let (top_slab_id, top_slab, top_slab_properties) = state(
            "minecraft:oak_slab",
            &[("type", "top"), ("waterlogged", "false")],
        );
        assert!(has_full_sturdy_face(
            top_slab_id,
            &top_slab,
            &top_slab_properties,
            SturdyFace::Up
        ));
        assert!(!has_full_sturdy_face(
            top_slab_id,
            &top_slab,
            &top_slab_properties,
            SturdyFace::North
        ));

        let (bottom_slab_id, bottom_slab, bottom_slab_properties) = state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        assert!(has_full_sturdy_face(
            bottom_slab_id,
            &bottom_slab,
            &bottom_slab_properties,
            SturdyFace::Down
        ));
        assert!(!has_full_sturdy_face(
            bottom_slab_id,
            &bottom_slab,
            &bottom_slab_properties,
            SturdyFace::Up
        ));

        let (top_stair_id, top_stair, top_stair_properties) = state(
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "top"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        );
        assert!(has_full_sturdy_face(
            top_stair_id,
            &top_stair,
            &top_stair_properties,
            SturdyFace::Up
        ));

        let (cobblestone_id, _, _) = state("minecraft:cobblestone", &[]);
        assert!(
            !has_full_sturdy_face(cobblestone_id, &stone, &stone_properties, SturdyFace::Up),
            "a numeric state id with the wrong block fingerprint must reject"
        );
    }
}
