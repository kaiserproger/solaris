use super::{Identifier, TerrainGenerator};

#[derive(Debug, Clone, Copy)]
pub struct TreeRule {
    pub(super) spacing: u64,
    pub(super) density_threshold: f64,
}

impl TreeRule {
    #[must_use]
    pub fn new(spacing: u64, density_threshold: f64) -> Option<Self> {
        (spacing > 0 && density_threshold.is_finite() && (-1.0..=1.0).contains(&density_threshold))
            .then_some(Self {
                spacing,
                density_threshold,
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClayRule {
    pub(super) rarity: u64,
    pub(super) radius_min: u8,
    pub(super) radius_max: u8,
    pub(super) max_water_depth: u8,
}

impl Default for ClayRule {
    fn default() -> Self {
        Self {
            rarity: 3,
            radius_min: 2,
            radius_max: 3,
            max_water_depth: 8,
        }
    }
}

impl ClayRule {
    #[must_use]
    pub fn new(rarity: u64, radius_min: u8, radius_max: u8, max_water_depth: u8) -> Option<Self> {
        (rarity > 0
            && radius_min > 0
            && radius_min <= radius_max
            && radius_max <= 3
            && (1..=32).contains(&max_water_depth))
        .then_some(Self {
            rarity,
            radius_min,
            radius_max,
            max_water_depth,
        })
    }
}

impl TerrainGenerator {
    pub fn define_tree_rule(
        &mut self,
        biome: Identifier,
        rule: TreeRule,
    ) -> Result<(), &'static str> {
        if !self.biomes.all.contains(&biome) {
            return Err("tree rule names an unavailable biome");
        }
        self.tree_rules.insert(biome, rule);
        Ok(())
    }

    pub fn define_clay_rule(&mut self, rule: ClayRule) -> Result<(), &'static str> {
        if self.decorations.clay.is_none() {
            return Err("clay rule requires minecraft:clay in the block registry");
        }
        self.clay_rule = rule;
        Ok(())
    }
}
