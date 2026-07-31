use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_data::Identifier;

use crate::anvil::{chunk_to_payload, write_region};
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{BlockPos, Chunk, ChunkPos, MAX_Y, MIN_Y};

use super::{REGION_AXIS_CHUNKS, WorldStorage, region_of};

pub(super) fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(rel)
}

pub(super) fn top_non_air_y(
    world: &mut WorldStorage,
    x: i32,
    z: i32,
    air: BlockStateId,
) -> Option<i32> {
    (MIN_Y..MAX_Y)
        .rev()
        .find(|&y| world.get_block(BlockPos { x, y, z }).ok().flatten() != Some(air))
}

pub(super) fn air_state_id(registry: &BlockRegistry) -> BlockStateId {
    registry
        .block(&Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .unwrap()
}

pub(super) fn single_air_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&[mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }])
        .unwrap(),
    )
}

pub(super) fn air_stone_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&[
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
        ])
        .unwrap(),
    )
}

pub(super) fn air_stone_chest_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&[
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:chest").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 2,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
        ])
        .unwrap(),
    )
}

pub(super) fn air_stone_furnace_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&[
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:furnace").unwrap(),
                properties: std::collections::BTreeMap::from([(
                    "lit".to_string(),
                    vec!["false".to_string(), "true".to_string()],
                )]),
                states: vec![
                    mc_data::blocks::BlockStateReport {
                        id: 2,
                        default: true,
                        properties: std::collections::BTreeMap::from([(
                            "lit".to_string(),
                            "false".to_string(),
                        )]),
                    },
                    mc_data::blocks::BlockStateReport {
                        id: 3,
                        default: false,
                        properties: std::collections::BTreeMap::from([(
                            "lit".to_string(),
                            "true".to_string(),
                        )]),
                    },
                ],
            },
        ])
        .unwrap(),
    )
}

pub(super) fn air_stone_hopper_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&[
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 2,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
        ])
        .unwrap(),
    )
}

pub(super) fn write_chunk_payload_at_slot(
    world_root: &Path,
    slot_pos: ChunkPos,
    embedded_pos: ChunkPos,
    registry: &BlockRegistry,
    trailing_bytes: &[u8],
) -> PathBuf {
    let region_root = world_root.join("region");
    std::fs::create_dir_all(&region_root).unwrap();
    let chunk = Chunk::empty(
        embedded_pos,
        air_state_id(registry),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let mut payload = chunk_to_payload(&chunk, registry, 1_700_000_000).unwrap();
    payload.local_x = slot_pos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
    payload.local_z = slot_pos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;
    payload.uncompressed_nbt.extend_from_slice(trailing_bytes);
    let (rx, rz) = region_of(slot_pos);
    let path = region_root.join(format!("r.{rx}.{rz}.mca"));
    write_region(&path, &[payload]).unwrap();
    path
}
