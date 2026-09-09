use mc_world::Chunk;

use super::{ColumnPlan, TerrainGenerator, feature_hash};

impl TerrainGenerator {
    pub(super) fn apply_sediments(&self, chunk: &mut Chunk, columns: &[ColumnPlan; 256]) {
        let Some(clay) = self.decorations.clay else {
            return;
        };
        let hash = feature_hash(self.seed, chunk.pos.x, 0, chunk.pos.z, 0xC1A7);
        if !hash.is_multiple_of(self.clay_rule.rarity) {
            return;
        }
        let center_x = 3 + (hash % 10) as i32;
        let center_z = 3 + ((hash >> 8) % 10) as i32;
        // Vanilla 26.1.2 disk_clay uses radius 2..=3 and half-height 1.
        let radius = i32::from(self.clay_rule.radius_min)
            + ((hash >> 16) % u64::from(self.clay_rule.radius_max - self.clay_rule.radius_min + 1))
                as i32;
        for z in center_z - radius..=center_z + radius {
            for x in center_x - radius..=center_x + radius {
                if (x - center_x).pow(2) + (z - center_z).pow(2) > radius * radius {
                    continue;
                }
                let plan = &columns[z as usize * 16 + x as usize];
                let depth = plan.top_non_air - plan.height;
                if !(1..=i32::from(self.clay_rule.max_water_depth)).contains(&depth)
                    || chunk.get_block(x as u8, plan.height + 1, z as u8) != Some(self.water)
                    || !(plan.surface == self.sand || plan.surface == self.dirt)
                {
                    continue;
                }
                for y in plan.height - 1..=plan.height {
                    if chunk
                        .get_block(x as u8, y, z as u8)
                        .is_some_and(|state| state == plan.surface || state == self.dirt)
                    {
                        chunk.set_block(x as u8, y, z as u8, clay);
                    }
                }
            }
        }
    }
}
