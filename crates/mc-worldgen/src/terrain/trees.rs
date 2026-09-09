use mc_data::Identifier;
use mc_world::Chunk;

use super::{
    ColumnPlan, TerrainGenerator, TreeBlocks, TreeKind, TreeLeafOffset, checked_y_offset,
    feature_hash, tree_canopy_radius,
};

impl TerrainGenerator {
    pub(super) fn place_tree(
        &self,
        chunk: &mut Chunk,
        plan: &ColumnPlan,
        blocks: Option<TreeBlocks>,
        touched: &mut [Option<i32>; 256],
        leaves: &mut Vec<(u8, i32, u8)>,
    ) -> bool {
        let lx = plan.lx;
        let lz = plan.lz;
        let Some(base_y) = checked_y_offset(plan.height, 1) else {
            return false;
        };
        let Some(mut blocks) = blocks else {
            return false;
        };
        let root_blocks = if blocks.kind == TreeKind::Mangrove {
            let (Some(dry), Some(wet), Some(muddy)) = (
                self.decorations.mangrove_roots,
                self.decorations.wet_mangrove_roots,
                self.decorations.muddy_mangrove_roots,
            ) else {
                return false;
            };
            Some((dry, wet, muddy))
        } else {
            None
        };
        let root_height = if root_blocks.is_some() {
            2 + ((plan.hash >> 18) & 1) as i32
        } else {
            0
        };
        let Some(trunk_base_y) = checked_y_offset(base_y, root_height) else {
            return false;
        };
        if blocks.kind == TreeKind::Jungle && (plan.hash >> 16).is_multiple_of(2) {
            let Some(leaves) = self.decorations.oak_leaves else {
                return false;
            };
            blocks.kind = TreeKind::JungleBush;
            blocks.leaves = leaves;
        }
        let trunk_height = match blocks.kind {
            TreeKind::Oak => 4 + (plan.hash % 2) as i32,
            TreeKind::Birch => 5 + (plan.hash % 2) as i32,
            TreeKind::Spruce => 5 + (plan.hash % 3) as i32,
            TreeKind::Jungle => 4 + ((plan.hash >> 8) % 9) as i32,
            TreeKind::JungleBush => 1,
            TreeKind::Acacia => 4 + (plan.hash % 3) as i32,
            TreeKind::Mangrove => 5 + ((plan.hash >> 8) % 4) as i32,
        };
        let Some(trunk_top_y) = checked_y_offset(trunk_base_y, trunk_height - 1) else {
            return false;
        };
        let crown_height = if blocks.kind == TreeKind::JungleBush {
            2
        } else {
            1
        };
        let Some(top_y) = checked_y_offset(trunk_top_y, crown_height) else {
            return false;
        };
        if !(2..=13).contains(&lx) || !(2..=13).contains(&lz) || top_y >= self.geometry.max_y() {
            return false;
        }
        let Some(support_y) = checked_y_offset(base_y, -1) else {
            return false;
        };
        if chunk.get_block(lx, support_y, lz) != Some(plan.surface) {
            return false;
        }
        let wet_site = self.biomes.swamp.contains(&plan.biome);
        for y in base_y..=top_y {
            let state = chunk.get_block(lx, y, lz);
            let shallow_water = wet_site && y - base_y <= 1 && state == Some(self.water);
            if state != Some(self.air) && !shallow_water {
                return false;
            }
        }
        if let Some((dry, wet, muddy)) = root_blocks {
            self.place_single(chunk, lx, plan.height, lz, muddy, touched);
            for y in base_y..trunk_base_y {
                let root = if chunk.get_block(lx, y, lz) == Some(self.water) {
                    wet
                } else {
                    dry
                };
                self.place_single(chunk, lx, y, lz, root, touched);
            }
            // Root arms attach to actual neighbouring soil. Length, height and
            // missing arms vary independently; no floating roots or square skirt.
            for (index, (dx, dz)) in [
                (1_i8, 0_i8),
                (1, 1),
                (0, 1),
                (-1, 1),
                (-1, 0),
                (-1, -1),
                (0, -1),
                (1, -1),
            ]
            .into_iter()
            .enumerate()
            {
                if (plan.hash >> (20 + index * 2)) & 3 == 0 {
                    continue;
                }
                let distance = 1 + ((plan.hash >> (36 + index)) & 1) as i8;
                let x = lx.wrapping_add_signed(dx * distance);
                let z = lz.wrapping_add_signed(dz * distance);
                let branch_y = trunk_base_y - 1 - ((plan.hash >> (44 + index)) & 1) as i32;
                let Some(soil_y) = ((plan.height - 1)..=plan.height + 1).rev().find(|&y| {
                    chunk.get_block(x, y, z).is_some_and(|state| {
                        Some(state) == self.decorations.mud
                            || state == self.grass_block
                            || state == self.dirt
                            || state == self.sand
                            || state == muddy
                    })
                }) else {
                    continue;
                };
                if soil_y >= branch_y {
                    continue;
                }
                let mut path = [(lx, lz); 4];
                let mut path_len = 0;
                for step in 1..=distance {
                    let next_x = lx.wrapping_add_signed(dx * step);
                    if dx != 0 {
                        path[path_len] = (next_x, lz.wrapping_add_signed(dz * (step - 1)));
                        path_len += 1;
                    }
                    if dz != 0 {
                        path[path_len] = (next_x, lz.wrapping_add_signed(dz * step));
                        path_len += 1;
                    }
                }
                let open = |state| {
                    state == Some(self.air)
                        || state == Some(self.water)
                        || state == Some(dry)
                        || state == Some(wet)
                };
                if !(soil_y + 1..branch_y).all(|y| open(chunk.get_block(x, y, z)))
                    || !path[..path_len]
                        .iter()
                        .all(|&(x, z)| open(chunk.get_block(x, branch_y, z)))
                {
                    continue;
                }
                self.place_single(chunk, x, soil_y, z, muddy, touched);
                for y in soil_y + 1..branch_y {
                    let state = chunk.get_block(x, y, z);
                    let root = if state == Some(self.water) || state == Some(wet) {
                        wet
                    } else {
                        dry
                    };
                    self.place_single(chunk, x, y, z, root, touched);
                }
                for &(x, z) in &path[..path_len] {
                    let state = chunk.get_block(x, branch_y, z);
                    let root = if state == Some(self.water) || state == Some(wet) {
                        wet
                    } else {
                        dry
                    };
                    self.place_single(chunk, x, branch_y, z, root, touched);
                }
            }
        }
        for y in trunk_base_y..=trunk_top_y {
            self.place_single(chunk, lx, y, lz, blocks.log, touched);
        }
        for relative_y in -4..=2 {
            let Some(radius) = tree_canopy_radius(blocks.kind, relative_y) else {
                continue;
            };
            let Some(y) = checked_y_offset(trunk_top_y, relative_y) else {
                continue;
            };
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let offset = TreeLeafOffset {
                        relative_y,
                        dx,
                        dz,
                        radius,
                    };
                    if !self.tree_leaf_is_present(plan, blocks.kind, trunk_top_y, offset) {
                        continue;
                    }
                    let x = lx.wrapping_add_signed(dx);
                    let z = lz.wrapping_add_signed(dz);
                    if chunk.get_block(x, y, z) == Some(self.air) {
                        self.place_single(chunk, x, y, z, blocks.leaves, touched);
                        leaves.push((x, y, z));
                    }
                }
            }
        }
        true
    }

    pub(super) fn tree_blocks_for_biome(&self, biome: &Identifier) -> Option<TreeBlocks> {
        let (kind, log, leaves) = if biome.path() == "mangrove_swamp" {
            (
                TreeKind::Mangrove,
                self.decorations.mangrove_log,
                self.decorations.mangrove_leaves,
            )
        } else if Self::is_savanna(biome) {
            (
                TreeKind::Acacia,
                self.decorations.acacia_log,
                self.decorations.acacia_leaves,
            )
        } else if self.biomes.jungle.contains(biome) {
            (
                TreeKind::Jungle,
                self.decorations.jungle_log,
                self.decorations.jungle_leaves,
            )
        } else if Self::is_cold_forest(biome) {
            (
                TreeKind::Spruce,
                self.decorations.cold_log,
                self.decorations.cold_leaves,
            )
        } else if self.biomes.temperate_forest.contains(biome) {
            (
                TreeKind::Birch,
                self.decorations.forest_log,
                self.decorations.forest_leaves,
            )
        } else {
            (
                TreeKind::Oak,
                self.decorations.oak_log,
                self.decorations.oak_leaves,
            )
        };
        log.zip(leaves)
            .map(|(log, leaves)| TreeBlocks { kind, log, leaves })
    }

    pub(super) fn tree_leaf_is_present(
        &self,
        plan: &ColumnPlan,
        kind: TreeKind,
        trunk_top_y: i32,
        offset: TreeLeafOffset,
    ) -> bool {
        let TreeLeafOffset {
            relative_y,
            dx,
            dz,
            radius,
        } = offset;
        let y = trunk_top_y + relative_y;
        if radius == 0 {
            return dx == 0 && dz == 0;
        }
        let edge_x = dx.unsigned_abs() == radius as u8;
        let edge_z = dz.unsigned_abs() == radius as u8;
        if edge_x && edge_z {
            if radius >= 2 {
                return false;
            }
            let corner = match (dx.is_positive(), dz.is_positive()) {
                (false, false) => 0,
                (true, false) => 1,
                (true, true) => 2,
                (false, true) => 3,
            };
            let salt = match kind {
                TreeKind::Oak => 0x0A4,
                TreeKind::Birch => 0xB17C,
                TreeKind::Spruce => 0x5A9C,
                TreeKind::Jungle | TreeKind::JungleBush => 0xA6E1,
                TreeKind::Acacia => 0xACA1,
                TreeKind::Mangrove => 0x4D41_4E47,
            };
            let rotation = feature_hash(self.seed, plan.wx, trunk_top_y, plan.wz, salt) as u8 & 3;
            return if relative_y > 0 {
                corner == rotation
            } else {
                corner != rotation
            };
        }
        if radius < 2 || !(edge_x || edge_z) {
            return true;
        }
        let salt = match kind {
            TreeKind::Oak => 0x0A4,
            TreeKind::Birch => 0xB17C,
            TreeKind::Spruce => 0x5A9C,
            TreeKind::Jungle | TreeKind::JungleBush => 0xA6E1,
            TreeKind::Acacia => 0xACA1,
            TreeKind::Mangrove => 0x4D41_4E47,
        };
        !feature_hash(
            self.seed,
            plan.wx + i32::from(dx),
            y,
            plan.wz + i32::from(dz),
            salt,
        )
        .is_multiple_of(5)
    }
}
