use super::*;
use mc_data::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;

fn nether_registry() -> Arc<BlockRegistry> {
    let state = |id: u32, properties: &[(&str, &str)]| BlockStateReport {
        id,
        default: true,
        properties: properties
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    let simple = |id: u32, name: &str| BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(id, &[])],
    };
    let report = vec![
        simple(0, "minecraft:air"),
        simple(1, "minecraft:bedrock"),
        simple(2, "minecraft:netherrack"),
        BlockReport {
            id: Identifier::parse("minecraft:lava").unwrap(),
            properties: BTreeMap::from([("level".to_string(), vec!["0".to_string()])]),
            states: vec![state(3, &[("level", "0")])],
        },
    ];
    Arc::new(BlockRegistry::from_report(&report).unwrap())
}

fn get(chunk: &Chunk, x: u8, y: i32, z: u8) -> BlockStateId {
    chunk
        .get_block(x, y, z)
        .expect("generated nether column is in-bounds")
}

#[test]
fn floor_and_ceiling_cap_every_column() {
    let generator = NetherGenerator::new(1234, nether_registry());
    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });
    for x in 0..16u8 {
        for z in 0..16u8 {
            assert_eq!(get(&chunk, x, NETHER_MIN_Y, z), BlockStateId(1));
            assert_eq!(get(&chunk, x, NETHER_CEILING_Y, z), BlockStateId(1));
            for y in NETHER_CEILING_Y - super::NETHER_CEILING_BAND..NETHER_CEILING_Y {
                let state = get(&chunk, x, y, z);
                assert!(
                    state == BlockStateId(1) || state == BlockStateId(2),
                    "ceiling band holds {state:?} at ({x}, {y}, {z})"
                );
            }
        }
    }
}

#[test]
fn body_is_lava_below_sea_netherrack_above_air_on_top() {
    let generator = NetherGenerator::new(1234, nether_registry());
    let mut saw_lava = false;
    let mut saw_netherrack = false;
    for cx in -4..4 {
        for cz in -4..4 {
            let chunk = generator.generate(ChunkPos { x: cx, z: cz });
            for x in 0..16u8 {
                for z in 0..16u8 {
                    let mut saw_column_lava = false;
                    let mut saw_column_rock = false;
                    for y in 1..NETHER_CEILING_Y - super::NETHER_CEILING_BAND {
                        match get(&chunk, x, y, z) {
                            BlockStateId(3) => {
                                assert!(y < NETHER_LAVA_LEVEL, "lava above sea level at {y}");
                                saw_column_lava = true;
                                saw_lava = true;
                            }
                            BlockStateId(2) => {
                                saw_column_rock = true;
                                saw_netherrack = true;
                            }
                            BlockStateId(0) => {}
                            other => panic!("unexpected nether body block {other:?} at {y}"),
                        }
                    }
                    assert!(
                        !(saw_column_lava && saw_column_rock),
                        "lava and netherrack mixed in column ({x}, {z}): buried lava"
                    );
                    for y in NETHER_CEILING_Y + 1..NETHER_MIN_Y + NETHER_HEIGHT {
                        assert_eq!(get(&chunk, x, y, z), BlockStateId(0));
                    }
                }
            }
        }
    }
    assert!(saw_lava, "lava sea never surfaces across 16 chunks");
    assert!(
        saw_netherrack,
        "netherrack body never surfaces across 16 chunks"
    );
}

#[test]
fn generation_is_deterministic_per_chunk() {
    let generator = NetherGenerator::new(99, nether_registry());
    let pos = ChunkPos { x: 3, z: -7 };
    let first = generator.generate(pos);
    // Interleave surrounding chunks and a second same-seed generator before
    // regenerating the target: shared-state contamination would show here.
    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx != 0 || dz != 0 {
                generator.generate(ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                });
            }
        }
    }
    let twin = NetherGenerator::new(99, nether_registry());
    twin.generate(ChunkPos { x: -40, z: 12 });
    let second = generator.generate(pos);
    for x in 0..16u8 {
        for z in 0..16u8 {
            for y in NETHER_MIN_Y..NETHER_MIN_Y + NETHER_HEIGHT {
                assert_eq!(
                    get(&first, x, y, z),
                    get(&second, x, y, z),
                    "chunk order changed column ({x}, {z}) at {y}"
                );
            }
        }
    }
}

#[test]
fn missing_netherrack_is_a_startup_error() {
    let state = |id: u32| BlockStateReport {
        id,
        default: true,
        properties: BTreeMap::new(),
    };
    let simple = |id: u32, name: &str| BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(id)],
    };
    let report = vec![
        simple(0, "minecraft:air"),
        simple(1, "minecraft:bedrock"),
        BlockReport {
            id: Identifier::parse("minecraft:lava").unwrap(),
            properties: BTreeMap::from([("level".to_string(), vec!["0".to_string()])]),
            states: vec![BlockStateReport {
                id: 2,
                default: true,
                properties: BTreeMap::from([("level".to_string(), "0".to_string())]),
            }],
        },
    ];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    assert!(matches!(
        NetherGenerator::try_new(7, registry),
        Err(NetherGeneratorError::MissingRequiredBlock(
            "minecraft:netherrack"
        ))
    ));
}
