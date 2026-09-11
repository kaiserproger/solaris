use super::*;
use mc_data::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;

fn end_registry() -> Arc<BlockRegistry> {
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
        simple(2, "minecraft:end_stone"),
        simple(3, "minecraft:obsidian"),
    ];
    Arc::new(BlockRegistry::from_report(&report).unwrap())
}

fn get(chunk: &Chunk, x: u8, y: i32, z: u8) -> BlockStateId {
    chunk
        .get_block(x, y, z)
        .expect("generated end column is in-bounds")
}

#[test]
fn island_body_is_end_stone_with_void_at_edges() {
    let generator = EndGenerator::new(1234, end_registry());
    // Origin chunk sits on the island: every column is solid end stone from
    // the floor up to a surface, with only air or pillar blocks above.
    // Pillar shafts start above the island surface, so rock and pillar
    // blocks never mix in one row.
    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });
    let mut saw_rock = false;
    for x in 0..16u8 {
        for z in 0..16u8 {
            let mut top_rock: Option<i32> = None;
            for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
                match get(&chunk, x, y, z) {
                    BlockStateId(2) => {
                        top_rock = Some(y);
                        saw_rock = true;
                    }
                    BlockStateId(0) | BlockStateId(1) | BlockStateId(3) => {}
                    other => panic!("unexpected end body block {other:?} at {y}"),
                }
            }
            if let Some(top) = top_rock {
                for y in END_MIN_Y..=top {
                    assert_eq!(
                        get(&chunk, x, y, z),
                        BlockStateId(2),
                        "island column ({x}, {z}) has a hole at {y}"
                    );
                }
                for y in top + 1..END_MIN_Y + END_HEIGHT {
                    assert!(
                        get(&chunk, x, y, z) != BlockStateId(2),
                        "end stone floats above surface at ({x}, {y}, {z})"
                    );
                }
            }
        }
    }
    assert!(saw_rock, "origin island has no end stone");
    // Far past the rim every column is void: all air, floor included.
    let far = generator.generate(ChunkPos { x: 64, z: -64 });
    for x in 0..16u8 {
        for z in 0..16u8 {
            for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
                assert_eq!(get(&far, x, y, z), BlockStateId(0));
            }
        }
    }
}

#[test]
fn pillar_ring_stands_near_origin_with_bedrock_caps() {
    let generator = EndGenerator::new(1234, end_registry());
    let mut saw_obsidian = false;
    let mut column_tops = Vec::new();
    // Ring radius 42 plus the 3-wide footprint fits inside ±3 chunks.
    for cx in -3..3 {
        for cz in -3..3 {
            let chunk = generator.generate(ChunkPos { x: cx, z: cz });
            for x in 0..16u8 {
                for z in 0..16u8 {
                    let mut top_obsidian: Option<i32> = None;
                    for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
                        match get(&chunk, x, y, z) {
                            BlockStateId(3) => {
                                top_obsidian = Some(y);
                                saw_obsidian = true;
                            }
                            BlockStateId(1) => {
                                // Every bedrock block is a pillar cap, so the
                                // block directly below it is shaft obsidian.
                                assert_eq!(
                                    get(&chunk, x, y - 1, z),
                                    BlockStateId(3),
                                    "bedrock without shaft below at ({x}, {y}, {z})"
                                );
                            }
                            BlockStateId(0) | BlockStateId(2) => {}
                            other => panic!("unexpected end pillar block {other:?} at {y}"),
                        }
                    }
                    if let Some(top) = top_obsidian {
                        column_tops.push(top);
                        // Each shaft is capped: bedrock directly above the top
                        // obsidian row.
                        assert_eq!(
                            get(&chunk, x, top + 1, z),
                            BlockStateId(1),
                            "pillar shaft uncapped at ({x}, {top}, {z})"
                        );
                    }
                }
            }
        }
    }
    assert!(saw_obsidian, "no obsidian pillars near the origin");
    assert!(
        column_tops.len() >= END_PILLAR_COUNT,
        "pillar ring under-populated: {} shaft columns",
        column_tops.len()
    );
}

#[test]
fn generation_is_deterministic_per_chunk() {
    let generator = EndGenerator::new(99, end_registry());
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
    let twin = EndGenerator::new(99, end_registry());
    twin.generate(ChunkPos { x: -40, z: 12 });
    let second = generator.generate(pos);
    for x in 0..16u8 {
        for z in 0..16u8 {
            for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
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
fn missing_end_stone_is_a_startup_error() {
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
        simple(2, "minecraft:obsidian"),
    ];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    assert!(matches!(
        EndGenerator::try_new(7, registry),
        Err(EndGeneratorError::MissingRequiredBlock(
            "minecraft:end_stone"
        ))
    ));
}
