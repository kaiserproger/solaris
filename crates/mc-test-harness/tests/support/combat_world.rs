use std::sync::Arc;

use mc_data::Identifier;
use mc_world::{BlockRegistry, WorldStorage};

pub(super) fn world(
    blocks: &Arc<BlockRegistry>,
    blocks_report: &[mc_data::blocks::BlockReport],
    view_distance: i32,
) -> WorldStorage {
    let state = |name: &str| {
        blocks
            .block(&Identifier::parse(name).unwrap())
            .expect("arena block")
            .default
    };
    let stone = state("minecraft:stone");
    let air = state("minecraft:air");
    let light =
        mc_data::block_light::BlockLightTable::conservative_from_blocks_report(blocks_report);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(blocks)));
    let mut storage = WorldStorage::in_memory_with_capacity(
        Arc::clone(blocks),
        ((2 * view_distance + 3) as usize).pow(2),
    )
    .with_generator(generator);
    // Exercise combat above generated hills and trees, with a published spawn
    // height and solid ground for splash impacts and village defenders.
    for x in -16..=16 {
        for z in -16..=32 {
            storage
                .set_block_at(mc_world::BlockPos { x, y: 199, z }, stone)
                .expect("arena floor");
            for y in 200..=216 {
                storage
                    .set_block_at(mc_world::BlockPos { x, y, z }, air)
                    .expect("clear combat space");
            }
            storage
                .update_highest_opaque_at(mc_world::BlockPos { x, y: 199, z }, &light)
                .expect("publish arena spawn height");
        }
    }
    storage
}
