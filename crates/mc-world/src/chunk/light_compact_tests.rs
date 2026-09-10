use super::*;
use crate::anvil::chunk_nbt::{chunk_from_nbt, chunk_to_nbt};
use crate::block::BlockRegistry;
use crate::wire::encode_chunk_light;

fn air_chunk() -> Chunk {
    Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    )
}

fn registry() -> BlockRegistry {
    BlockRegistry::from_report(&[mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    }])
    .unwrap()
}

#[test]
fn known_zero_baked_light_survives_disk_roundtrip() {
    let registry = registry();
    let mut chunk = air_chunk();
    assert!(ChunkLight::from_chunk(&chunk).is_none());
    chunk.set_baked_light(&ChunkLight::zeroed());
    assert!(chunk.section_lights.iter().all(|section| {
        section.sky.as_ref().is_some_and(LightSection::is_zero)
            && section.block.as_ref().is_some_and(LightSection::is_zero)
    }));
    let decoded = chunk_from_nbt(&chunk_to_nbt(&chunk, &registry).unwrap(), &registry).unwrap();
    assert_eq!(ChunkLight::from_chunk(&decoded), Some(ChunkLight::zeroed()));
    assert_eq!(decoded.section_lights, chunk.section_lights);
    assert!(decoded.section_lights.iter().all(|section| {
        section.sky.as_ref().unwrap().allocated_bytes() == 0
            && section.block.as_ref().unwrap().allocated_bytes() == 0
    }));
}

#[test]
fn stored_and_computed_snapshots_survive_cow_writes() {
    let mut chunk = air_chunk();
    chunk.section_lights[0].sky = Some(LightSection::from_bytes(std::array::from_fn(|index| {
        index as u8
    })));
    let snapshot = chunk.clone();
    let mut computed = ChunkLight::from_chunk(&chunk).unwrap();
    let computed_snapshot = computed.clone();
    let original = snapshot.section_lights[0].sky.as_ref().unwrap();
    assert_eq!(
        original.shared_allocation(),
        computed.sky.section(0).unwrap().shared_allocation()
    );
    chunk.section_lights[0].sky.as_mut().unwrap().set(0, 7);
    computed.sky.set(1, 0, 0, 9);
    assert_eq!(original.byte(0), 0);
    assert_eq!(computed_snapshot.sky.section(0).unwrap().byte(0), 0);
    assert_eq!(chunk.section_lights[0].sky.as_ref().unwrap().byte(0), 0x07);
    assert_eq!(computed.sky.section(0).unwrap().byte(0), 0x90);

    let mut uniform = LightSection::uniform(0xFF);
    let uniform_snapshot = uniform.clone();
    assert_eq!(uniform.allocated_bytes(), 0);
    uniform.set(4095, 3);
    assert_eq!(uniform.get(4094), 15);
    assert_eq!(uniform.get(4095), 3);
    assert_eq!(uniform_snapshot.get(4095), 15);

    let mut zero = LightSection::uniform(0);
    zero.set(1, 4);
    let lit_snapshot = zero.clone();
    zero.set(1, 0);
    assert!(zero.is_zero());
    assert_eq!(zero.allocated_bytes(), 0);
    assert_eq!(lit_snapshot.get(1), 4);
}

#[test]
fn mixed_uniform_and_unknown_layers_preserve_disk_and_wire_values() {
    let registry = registry();
    let mut chunk = air_chunk();
    let mixed = std::array::from_fn(|index| (index as u8).wrapping_mul(37));
    chunk.section_lights[0].sky = Some(LightSection::uniform(0xFF));
    chunk.section_lights[0].block = Some(LightSection::uniform(0));
    chunk.section_lights[1].block = Some(LightSection::from_bytes(mixed));
    chunk.section_lights[2].sky = Some(LightSection::uniform(0x21));
    let root = chunk_to_nbt(&chunk, &registry).unwrap();
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &root).unwrap();
    let (_, root) = mc_nbt::read_named(&mut std::io::Cursor::new(bytes)).unwrap();
    let decoded = chunk_from_nbt(&root, &registry).unwrap();
    assert_eq!(decoded.section_lights, chunk.section_lights);
    assert_eq!(decoded.section_lights[1].sky, None);
    assert!(decoded.section_lights[0].block.as_ref().unwrap().is_zero());

    let wire = encode_chunk_light(&ChunkLight::from_chunk(&decoded).unwrap());
    let top_slot = SECTION_COUNT + 1;
    let all_slots = (1_i64 << (SECTION_COUNT + 2)) - 1;
    assert_eq!(wire.sky_y_mask, vec![(1 << 1) | (1 << 3) | (1 << top_slot)]);
    assert_eq!(wire.block_y_mask, vec![1 << 2]);
    assert_eq!(wire.empty_sky_y_mask, vec![all_slots ^ wire.sky_y_mask[0]]);
    assert_eq!(wire.empty_block_y_mask, vec![all_slots ^ (1 << 2)]);
    assert_eq!(
        wire.sky_updates,
        vec![
            vec![0xFF; LIGHT_LAYER_BYTES],
            vec![0x21; LIGHT_LAYER_BYTES],
            vec![0xFF; LIGHT_LAYER_BYTES]
        ]
    );
    assert_eq!(wire.block_updates, vec![mixed.to_vec()]);
    let received = LightSection::from_bytes(wire.block_updates[0].clone().try_into().unwrap());
    for cell in 0..LIGHT_LAYER_BYTES * 2 {
        assert_eq!(
            received.get(cell),
            (mixed[cell / 2] >> ((cell & 1) * 4)) & 15
        );
    }
}
