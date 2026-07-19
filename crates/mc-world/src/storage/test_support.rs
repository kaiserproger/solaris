use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_data::Identifier;

use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{BlockPos, MAX_Y, MIN_Y};

use super::WorldStorage;

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
