use mc_world::chunk::Chunk;
use mc_world::{BlockRegistry, BlockStateId};

use super::{TerrainGenerator, feature_hash, resolve_block_or};

#[derive(Debug, Clone)]
pub(super) struct GeologicalOreRules {
    rules: Vec<GeologicalOreRule>,
}

#[derive(Debug, Clone, Copy)]
struct GeologicalOreRule {
    normal: BlockStateId,
    deepslate: BlockStateId,
    min_y: i32,
    max_y: i32,
    cell_size: i32,
    long_radius: i32,
    short_radius: i32,
    vertical_radius: i32,
    salt: u64,
}

struct OreNames {
    normal: &'static str,
    deepslate: &'static str,
    min_y: i32,
    max_y: i32,
    cell_size: i32,
    long_radius: i32,
    short_radius: i32,
    vertical_radius: i32,
}

const GEOLOGICAL_ORES: &[OreNames] = &[
    OreNames {
        normal: "minecraft:coal_ore",
        deepslate: "minecraft:deepslate_coal_ore",
        min_y: 0,
        max_y: 192,
        cell_size: 72,
        long_radius: 22,
        short_radius: 9,
        vertical_radius: 6,
    },
    OreNames {
        normal: "minecraft:iron_ore",
        deepslate: "minecraft:deepslate_iron_ore",
        min_y: -48,
        max_y: 96,
        cell_size: 64,
        long_radius: 20,
        short_radius: 8,
        vertical_radius: 6,
    },
    OreNames {
        normal: "minecraft:copper_ore",
        deepslate: "minecraft:deepslate_copper_ore",
        min_y: -16,
        max_y: 112,
        cell_size: 80,
        long_radius: 20,
        short_radius: 9,
        vertical_radius: 7,
    },
    OreNames {
        normal: "minecraft:gold_ore",
        deepslate: "minecraft:deepslate_gold_ore",
        min_y: -64,
        max_y: 32,
        cell_size: 128,
        long_radius: 16,
        short_radius: 7,
        vertical_radius: 5,
    },
    OreNames {
        normal: "minecraft:redstone_ore",
        deepslate: "minecraft:deepslate_redstone_ore",
        min_y: -64,
        max_y: 15,
        cell_size: 112,
        long_radius: 18,
        short_radius: 7,
        vertical_radius: 5,
    },
    OreNames {
        normal: "minecraft:diamond_ore",
        deepslate: "minecraft:deepslate_diamond_ore",
        min_y: -64,
        max_y: 16,
        cell_size: 192,
        long_radius: 13,
        short_radius: 6,
        vertical_radius: 4,
    },
    OreNames {
        normal: "minecraft:lapis_ore",
        deepslate: "minecraft:deepslate_lapis_ore",
        min_y: -64,
        max_y: 64,
        cell_size: 144,
        long_radius: 15,
        short_radius: 6,
        vertical_radius: 5,
    },
    OreNames {
        normal: "minecraft:emerald_ore",
        deepslate: "minecraft:deepslate_emerald_ore",
        min_y: -16,
        max_y: 240,
        cell_size: 224,
        long_radius: 12,
        short_radius: 5,
        vertical_radius: 4,
    },
];

impl GeologicalOreRules {
    pub(super) fn new(registry: &BlockRegistry, fallback: BlockStateId) -> Self {
        let rules = GEOLOGICAL_ORES
            .iter()
            .enumerate()
            .map(|(index, rule)| GeologicalOreRule {
                normal: resolve_block_or(registry, rule.normal, fallback),
                deepslate: resolve_block_or(registry, rule.deepslate, fallback),
                min_y: rule.min_y,
                max_y: rule.max_y,
                cell_size: rule.cell_size,
                long_radius: rule.long_radius,
                short_radius: rule.short_radius,
                vertical_radius: rule.vertical_radius,
                salt: 0x6E0_10A1_u64 ^ index as u64,
            })
            .collect();
        Self { rules }
    }

    pub(super) fn apply(&self, generator: &TerrainGenerator, chunk: &mut Chunk) {
        let chunk_min_x = i64::from(chunk.pos.x) * 16;
        let chunk_min_z = i64::from(chunk.pos.z) * 16;
        let chunk_max_x = chunk_min_x + 15;
        let chunk_max_z = chunk_min_z + 15;

        for rule in &self.rules {
            let halo = i64::from(rule.long_radius);
            let cell_size = i64::from(rule.cell_size);
            let min_cell_x = (chunk_min_x - halo).div_euclid(cell_size);
            let max_cell_x = (chunk_max_x + halo).div_euclid(cell_size);
            let min_cell_z = (chunk_min_z - halo).div_euclid(cell_size);
            let max_cell_z = (chunk_max_z + halo).div_euclid(cell_size);

            for cell_z in min_cell_z..=max_cell_z {
                for cell_x in min_cell_x..=max_cell_x {
                    let (Ok(cell_x_i32), Ok(cell_z_i32)) =
                        (i32::try_from(cell_x), i32::try_from(cell_z))
                    else {
                        continue;
                    };
                    let hash = feature_hash(
                        generator.seed,
                        cell_x_i32,
                        rule.min_y,
                        cell_z_i32,
                        rule.salt,
                    );
                    let anchor_x = cell_x * cell_size
                        + i64::try_from(hash % rule.cell_size as u64)
                            .expect("bounded deposit offset");
                    let anchor_z = cell_z * cell_size
                        + i64::try_from(hash.rotate_left(23) % rule.cell_size as u64)
                            .expect("bounded deposit offset");
                    let height = i64::from(rule.max_y - rule.min_y + 1);
                    let anchor_y = i64::from(rule.min_y)
                        + i64::try_from(hash.rotate_left(41) % height as u64)
                            .expect("bounded deposit height");
                    self.place_deposit(
                        generator,
                        chunk,
                        rule,
                        [anchor_x, anchor_y, anchor_z],
                        hash,
                    );
                }
            }
        }
    }

    fn place_deposit(
        &self,
        generator: &TerrainGenerator,
        chunk: &mut Chunk,
        rule: &GeologicalOreRule,
        anchor: [i64; 3],
        hash: u64,
    ) {
        let chunk_min_x = i64::from(chunk.pos.x) * 16;
        let chunk_min_z = i64::from(chunk.pos.z) * 16;
        let min_x = (anchor[0] - i64::from(rule.long_radius)).max(chunk_min_x);
        let max_x = (anchor[0] + i64::from(rule.long_radius)).min(chunk_min_x + 15);
        let min_z = (anchor[2] - i64::from(rule.long_radius)).max(chunk_min_z);
        let max_z = (anchor[2] + i64::from(rule.long_radius)).min(chunk_min_z + 15);
        if min_x > max_x || min_z > max_z {
            return;
        }
        let min_y = (anchor[1] - i64::from(rule.vertical_radius))
            .max(i64::from(generator.geometry.min_y() + 1));
        let max_y = (anchor[1] + i64::from(rule.vertical_radius))
            .min(i64::from(generator.geometry.max_y() - 1));
        let diagonal = hash & 1 != 0;

        for world_y in min_y..=max_y {
            for world_z in min_z..=max_z {
                for world_x in min_x..=max_x {
                    let dx = (world_x - anchor[0]) as f64;
                    let dz = (world_z - anchor[2]) as f64;
                    let (long, short) = if diagonal {
                        (
                            (dx + dz) * std::f64::consts::FRAC_1_SQRT_2,
                            (dx - dz) * std::f64::consts::FRAC_1_SQRT_2,
                        )
                    } else {
                        (dx, dz)
                    };
                    let dy = (world_y - anchor[1]) as f64;
                    let distance = long * long / f64::from(rule.long_radius.pow(2))
                        + short * short / f64::from(rule.short_radius.pow(2))
                        + dy * dy / f64::from(rule.vertical_radius.pow(2));
                    if distance > 1.0 {
                        continue;
                    }
                    let (Ok(wx), Ok(wy), Ok(wz)) = (
                        i32::try_from(world_x),
                        i32::try_from(world_y),
                        i32::try_from(world_z),
                    ) else {
                        continue;
                    };
                    let cell_hash = feature_hash(generator.seed, wx, wy, wz, rule.salt ^ hash);
                    if distance > 0.72 && cell_hash.is_multiple_of(5) {
                        continue;
                    }
                    let lx = (world_x - chunk_min_x) as u8;
                    let lz = (world_z - chunk_min_z) as u8;
                    let Some(base) = chunk.get_block(lx, wy, lz) else {
                        continue;
                    };
                    if base != generator.stone && base != generator.deepslate {
                        continue;
                    }
                    let ore = if base == generator.deepslate {
                        rule.deepslate
                    } else {
                        rule.normal
                    };
                    let _ = chunk.set_block(lx, wy, lz, ore);
                }
            }
        }
    }
}
