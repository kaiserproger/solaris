use super::*;
use crate::{BlockStateId, ChunkPos, LIGHT_LAYER_BYTES, LightSection};

#[test]
fn shared_block_and_light_payloads_are_charged_once_across_snapshots() {
    let mut first = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    first.sections[0].set(1, 1, 1, BlockStateId(1));
    let mut bytes = [0; LIGHT_LAYER_BYTES];
    bytes[17] = 0xF1;
    first.section_lights[0].sky = Some(LightSection::from_bytes(bytes));
    let mut second = first.clone();
    second.pos.x = 1;
    let shared = first.sections[0].shared_heap_allocation().unwrap().2
        + first.section_lights[0]
            .sky
            .as_ref()
            .unwrap()
            .allocated_bytes();
    let mut profile = ChunkMemoryProfile::default();
    profile.observe(&first);
    profile.observe(&second);
    assert_eq!(profile.shared_bytes_deduplicated, shared);
    assert_eq!(
        profile.unique_allocated_bytes + shared,
        profile.reachable_bytes
    );
    assert_eq!(
        profile.categories.values().sum::<usize>(),
        profile.unique_allocated_bytes
    );

    // Editing one snapshot detaches its payloads; they now need separate charges.
    second.sections[0].set(2, 1, 1, BlockStateId(1));
    second.section_lights[0].sky.as_mut().unwrap().set(0, 15);
    let mut detached = ChunkMemoryProfile::default();
    detached.observe(&first);
    detached.observe(&second);
    assert_eq!(detached.shared_bytes_deduplicated, 0);
    assert_eq!(detached.unique_allocated_bytes, detached.reachable_bytes);
}
