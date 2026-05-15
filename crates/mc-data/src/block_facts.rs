//! Narrow block-behaviour facts derived from the block report.

use crate::blocks::BlockReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomTickFamily {
    Crop,
    Farmland,
    Fire,
    Grass,
    Leaves,
    Sapling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockFacts {
    pub random_tick_family: Option<RandomTickFamily>,
}

#[derive(Debug, Clone, Default)]
pub struct BlockFactsTable {
    states: Vec<BlockFacts>,
    eligible_states: usize,
}

impl BlockFactsTable {
    #[must_use]
    pub fn from_blocks_report(report: &[BlockReport]) -> Self {
        let max_state = report
            .iter()
            .flat_map(|block| block.states.iter().map(|state| state.id as usize))
            .max()
            .unwrap_or(0);
        let mut states = vec![BlockFacts::default(); max_state.saturating_add(1)];
        let mut eligible_states = 0;
        for block in report {
            let family = classify_random_tick_family(block.id.path());
            for state in &block.states {
                let facts = &mut states[state.id as usize];
                if facts.random_tick_family.is_none() && family.is_some() {
                    eligible_states += 1;
                }
                facts.random_tick_family = family;
            }
        }
        Self {
            states,
            eligible_states,
        }
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
    fn loads_real_random_tick_families_when_present() {
        let path = workspace_path("data/vanilla/reports/blocks.json");
        if !path.is_file() {
            eprintln!(
                "skipping: {} not present (run tools/extract-vanilla-data.sh)",
                path.display()
            );
            return;
        }
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
}
